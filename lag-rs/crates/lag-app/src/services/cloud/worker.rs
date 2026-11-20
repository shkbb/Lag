//! The cloud worker (docs/spec/cloud.md §8): one background thread that runs the transfer queue, one job at a
//! time, and keeps a local copy of every account's cloud index (so the library shows cloud clips at once, even
//! offline). It owns the queue; the UI talks to it with `Cmd`s and hears back through `Update`s.
//!
//! Recording comes first: with "when no game is running" an upload stops between chunks as soon as a game
//! starts (the queue keeps its resumable session) and goes on when the game is closed.
use super::{facts, providers, stream};
use chrono::Utc;
use lag_capture::detect::GameDetector;
use lag_cloud::index::CloudIndex;
use lag_cloud::presence::Transfer;
use lag_cloud::provider::{CloudError, CloudProvider, CloudResult};
use lag_cloud::queue::{Job, JobKind, JobState, Queue};
use lag_cloud::settings::{AccountSettings, ClipFacts, CloudSettings, Trigger, UploadWhen};
use lag_cloud::sync::{AccountClient, UploadRequest};
use lag_cloud::{AccountId, ClipId};
use lag_core::metadata::ClipMetadataStore;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// What the UI asks of the worker.
pub(crate) enum Cmd {
    Settings(CloudSettings),
    /// A replay or screenshot was written: the automatic upload rule decides.
    ClipSaved(String),
    /// "Upload to the cloud" on a clip.
    UploadNow(String),
    Favorite { path: Option<String>, id: Option<ClipId>, favorite: bool },
    /// Remove a clip from every cloud that has it.
    DeleteFromCloud(ClipId),
    /// "Save to this PC": download into `dir`.
    Download { id: ClipId, dir: PathBuf },
    Retry,
    Refresh,
    SignedIn(AccountId),
}

/// What the worker tells the UI (delivered on the worker thread; the service forwards to the UI thread).
pub(crate) enum Update {
    Indexes(Vec<(AccountId, CloudIndex)>),
    Transfers(HashMap<(ClipId, AccountId), Transfer>),
    /// Live progress of the running transfer (0..=1).
    Progress { clip: ClipId, account: AccountId, fraction: f32, download: bool },
    /// None = the account works again.
    AccountError(AccountId, Option<&'static str>),
    /// A clip got its cloud id (written to its metadata).
    IdAssigned { path: String, id: ClipId },
    /// A clip came down from the cloud (the library rescans its folder).
    Downloaded,
    /// Thumbnails of cloud clips were fetched into the thumbnail cache.
    ThumbnailsReady,
}

pub(crate) type Out = Box<dyn Fn(Update) + Send>;

/// The cloud index is read again this often (other PCs upload too).
const INDEX_REFRESH: Duration = Duration::from_secs(600);
const GAME_CHECK: Duration = Duration::from_secs(15);
/// Thumbnails fetched per index refresh (small files; the rest come with the next refresh).
const THUMBS_PER_REFRESH: usize = 60;

/// `%LOCALAPPDATA%\Lag\cloud`: the queue and the index copies.
#[derive(Clone)]
pub(crate) struct Paths {
    pub dir: PathBuf,
}

impl Paths {
    pub fn for_user() -> Option<Self> {
        lag_core::hardware::local_appdata().map(|p| Self { dir: p.join("Lag").join("cloud") })
    }
    fn queue(&self) -> PathBuf {
        self.dir.join("queue.json")
    }
    fn index(&self, account: &AccountId) -> PathBuf {
        let name: String =
            account.as_str().chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
        self.dir.join(format!("index-{name}.json"))
    }
}

pub(crate) fn spawn(settings: CloudSettings, paths: Paths, out: Out) -> Sender<Cmd> {
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new().name("lag-cloud".into()).spawn(move || {
        Worker::new(settings, paths, out).run(rx);
    });
    tx
}

/// Is a game running? Asked at most every 15 s (the detector walks every window).
#[derive(Default)]
struct GameWatch {
    detector: Option<GameDetector>,
    checked: Option<Instant>,
    running: bool,
}

impl GameWatch {
    fn running(&mut self) -> bool {
        if self.checked.is_none_or(|t| t.elapsed() >= GAME_CHECK) {
            let detector = self.detector.get_or_insert_with(GameDetector::new);
            self.running = detector.find_best_game(None).is_some();
            self.checked = Some(Instant::now());
        }
        self.running
    }
}

struct Worker {
    settings: CloudSettings,
    paths: Paths,
    queue: Queue,
    indexes: HashMap<AccountId, CloudIndex>,
    providers: HashMap<AccountId, Arc<dyn CloudProvider>>,
    meta: Option<ClipMetadataStore>,
    game: GameWatch,
    out: Out,
    last_refresh: Option<Instant>,
    /// Commands that arrived during a transfer, handled right after it.
    deferred: Vec<Cmd>,
}

fn device_name() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "PC".into())
}

impl Worker {
    fn new(settings: CloudSettings, paths: Paths, out: Out) -> Self {
        let queue = std::fs::read_to_string(paths.queue()).ok().and_then(|t| Queue::from_json(&t).ok()).unwrap_or_default();
        let indexes = settings
            .accounts
            .iter()
            .filter_map(|a| {
                let text = std::fs::read_to_string(paths.index(&a.id)).ok()?;
                Some((a.id.clone(), CloudIndex::from_json(&text).ok()?))
            })
            .collect();
        Self {
            settings,
            paths,
            queue,
            indexes,
            providers: HashMap::new(),
            meta: ClipMetadataStore::for_user(),
            game: GameWatch::default(),
            out,
            last_refresh: None,
            deferred: Vec::new(),
        }
    }

    fn run(mut self, rx: Receiver<Cmd>) {
        self.publish_indexes();
        self.publish_transfers();
        loop {
            for cmd in std::mem::take(&mut self.deferred) {
                self.handle(cmd);
            }
            let wait = if self.next_job().is_some() { Duration::ZERO } else { Duration::from_secs(5) };
            match rx.recv_timeout(wait) {
                Ok(cmd) => self.handle(cmd),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            while let Ok(cmd) = rx.try_recv() {
                self.handle(cmd);
            }
            if self.last_refresh.is_none_or(|t| t.elapsed() >= INDEX_REFRESH) {
                self.refresh_indexes();
            }
            if let Some(id) = self.next_job() {
                self.run_job(id, &rx);
            }
        }
    }

    fn account(&self, id: &AccountId) -> Option<&AccountSettings> {
        self.settings.accounts.iter().find(|a| &a.id == id)
    }

    fn provider(&mut self, account: &AccountSettings) -> CloudResult<Arc<dyn CloudProvider>> {
        if let Some(p) = self.providers.get(&account.id) {
            return Ok(p.clone());
        }
        let p = providers::open(account)?;
        self.providers.insert(account.id.clone(), p.clone());
        Ok(p)
    }

    fn client<'a>(&self, provider: &'a dyn CloudProvider) -> AccountClient<'a> {
        AccountClient { provider, root: self.settings.root_folder.clone(), sort: self.settings.sort, device: device_name() }
    }

    fn save_queue(&self) {
        let _ = std::fs::create_dir_all(&self.paths.dir);
        if let Err(e) = std::fs::write(self.paths.queue(), self.queue.to_json()) {
            log::warn!("[Cloud] saving the queue failed: {e}");
        }
    }

    /// The next job allowed to run now (uploads wait while a game runs, if the user said so).
    fn next_job(&mut self) -> Option<u64> {
        let pending_upload = self.queue.jobs().iter().any(|j| j.kind == JobKind::Upload && j.state == JobState::Pending);
        let game = pending_upload && self.settings.upload_when == UploadWhen::NoGameRunning && self.game.running();
        let known: Vec<AccountId> = self.settings.accounts.iter().map(|a| a.id.clone()).collect();
        self.queue.next_ready(Utc::now(), |account, kind| (kind == JobKind::Upload && game) || !known.contains(account))
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Settings(settings) => {
                for gone in self.settings.accounts.iter().filter(|a| !settings.accounts.iter().any(|b| b.id == a.id)) {
                    self.queue.forget_account(&gone.id);
                    self.indexes.remove(&gone.id);
                    self.providers.remove(&gone.id);
                    let _ = std::fs::remove_file(self.paths.index(&gone.id));
                }
                let new_account = settings.accounts.iter().any(|a| !self.indexes.contains_key(&a.id));
                self.settings = settings;
                if new_account {
                    self.last_refresh = None;
                }
                self.publish_indexes();
            }
            Cmd::ClipSaved(path) => self.enqueue_upload(&path, Trigger::Automatic),
            Cmd::UploadNow(path) => self.enqueue_upload(&path, Trigger::Manual),
            Cmd::Favorite { path, id, favorite } => {
                let id = id.or_else(|| path.as_deref().and_then(|p| self.meta.as_ref().and_then(|m| facts::known_id(m, p))));
                if let Some(id) = id {
                    let holders: Vec<AccountId> = self.holders(id);
                    for account in holders {
                        let job = self.queue.push(id, &account, JobKind::SyncMeta, None);
                        self.queue.set_favorite(job, favorite);
                    }
                }
                // a new favorite may now belong in the cloud ("upload favorites", favorite mirrors)
                if let (true, Some(path)) = (favorite, path) {
                    self.enqueue_upload(&path, Trigger::Automatic);
                }
            }
            Cmd::DeleteFromCloud(id) => {
                for account in self.holders(id) {
                    self.queue.push(id, &account, JobKind::Delete, None);
                }
            }
            Cmd::Download { id, dir } => {
                let found = self.indexes.iter().find_map(|(a, i)| {
                    i.entries.get(&id).filter(|e| e.is_live()).map(|e| (a.clone(), e.file_name.clone()))
                });
                if let Some((account, name)) = found {
                    let dest = facts::unique_path(&dir, &name);
                    self.queue.push(id, &account, JobKind::Download, Some(dest.to_string_lossy().into_owned()));
                }
            }
            Cmd::Retry => {
                let failed: Vec<u64> =
                    self.queue.jobs().iter().filter(|j| matches!(j.state, JobState::Failed(_))).map(|j| j.id).collect();
                for id in failed {
                    self.queue.retry(id);
                }
            }
            Cmd::Refresh => self.last_refresh = None,
            Cmd::SignedIn(account) => {
                self.queue.signed_in(&account);
                self.providers.remove(&account);
                (self.out)(Update::AccountError(account, None));
                self.last_refresh = None;
            }
        }
        self.save_queue();
        self.publish_transfers();
    }

    /// Accounts whose cloud holds the clip.
    fn holders(&self, id: ClipId) -> Vec<AccountId> {
        self.indexes
            .iter()
            .filter(|(_, i)| i.entries.get(&id).is_some_and(|e| e.is_live()))
            .map(|(a, _)| a.clone())
            .collect()
    }

    fn enqueue_upload(&mut self, path: &str, trigger: Trigger) {
        if stream::is_cloud_key(path) {
            return;
        }
        let Some(meta) = self.meta.as_ref() else { return };
        let favorite = meta.read(path).is_some_and(|m| m.favorite);
        let targets: Vec<AccountId> = self
            .settings
            .targets(ClipFacts { favorite, is_image: facts::is_image(path) }, trigger)
            .into_iter()
            .cloned()
            .collect();
        if targets.is_empty() {
            return;
        }
        let Ok(size) = std::fs::metadata(path).map(|m| m.len()) else { return };
        let had_id = facts::known_id(meta, path);
        let id = facts::ensure_id(meta, path);
        if had_id.is_none() {
            (self.out)(Update::IdAssigned { path: path.to_owned(), id });
        }
        for account in targets {
            let already = self.indexes.get(&account).and_then(|i| i.entries.get(&id)).is_some_and(|e| e.is_live() && e.size == size);
            if !already {
                self.queue.push(id, &account, JobKind::Upload, Some(path.to_owned()));
            }
        }
    }

    fn run_job(&mut self, id: u64, rx: &Receiver<Cmd>) {
        let Some(job) = self.queue.get(id).cloned() else { return };
        let Some(account) = self.account(&job.account).cloned() else {
            self.queue.complete(id);
            return;
        };
        let provider = match self.provider(&account) {
            Ok(p) => p,
            Err(e) => {
                self.failed(&job, e);
                self.save_queue();
                self.publish_transfers();
                return;
            }
        };
        self.queue.start(id);
        self.save_queue();
        self.publish_transfers();
        let result = match job.kind {
            JobKind::Upload => self.upload(provider.as_ref(), &account, &job, rx),
            JobKind::Download => self.download(provider.as_ref(), &job, rx),
            JobKind::Delete => self.client(provider.as_ref()).delete_clip(job.clip),
            JobKind::SyncMeta => {
                let favorite = job.favorite;
                self.client(provider.as_ref()).sync_meta(job.clip, &move |e| {
                    if let Some(f) = favorite {
                        e.favorite = f;
                    }
                })
            }
        };
        match result {
            Ok(()) => {
                self.queue.complete(id);
                log::info!("[Cloud] {:?} {} on {} done", job.kind, job.clip, job.account);
                self.reload_index(provider.as_ref(), &account.id);
            }
            Err(e) => self.failed(&job, e),
        }
        self.save_queue();
        self.publish_transfers();
    }

    fn upload(
        &mut self,
        provider: &dyn CloudProvider,
        account: &AccountSettings,
        job: &Job,
        rx: &Receiver<Cmd>,
    ) -> CloudResult<()> {
        let path = job.local_path.clone().ok_or(CloudError::NotFound)?;
        let (entry, thumbnail) = facts::read(self.meta.as_ref(), &path).ok_or(CloudError::NotFound)?;
        let size = entry.size;
        let client = self.client(provider);
        // the cloud's auto-cleanup (only if switched on, only for the user's own limit) and the space check
        client.prepare_upload(account.auto_cleanup, account.max_size_gb, size)?;
        let mut file = std::fs::File::open(&path).map_err(|_| CloudError::NotFound)?;
        let pause_for_games = self.settings.upload_when == UploadWhen::NoGameRunning;
        let Worker { queue, paths, game, deferred, out, .. } = &mut *self;
        let out = &*out;
        let (job_id, clip, account_id) = (job.id, job.clip, job.account.clone());
        let mut last = Instant::now();
        client.upload_clip(
            UploadRequest { id: clip, entry, source: &mut file, thumbnail_jpeg: thumbnail },
            job.session.as_ref(),
            &mut |session| {
                // a restart (or a crash) resumes from the last confirmed chunk
                queue.save_session(job_id, session);
                let _ = std::fs::write(paths.queue(), queue.to_json());
            },
            &mut |done| {
                if last.elapsed() >= Duration::from_millis(400) || done >= size {
                    last = Instant::now();
                    let fraction = (done as f64 / size.max(1) as f64) as f32;
                    out(Update::Progress { clip, account: account_id.clone(), fraction, download: false });
                }
                deferred.extend(rx.try_iter());
                let stop_for_delete = deferred.iter().any(|c| matches!(c, Cmd::DeleteFromCloud(id) if *id == clip));
                !(stop_for_delete || (pause_for_games && game.running()))
            },
        )?;
        Ok(())
    }

    fn download(&mut self, provider: &dyn CloudProvider, job: &Job, rx: &Receiver<Cmd>) -> CloudResult<()> {
        let dest = PathBuf::from(job.local_path.clone().ok_or(CloudError::NotFound)?);
        let entry = self
            .indexes
            .get(&job.account)
            .and_then(|i| i.entries.get(&job.clip))
            .filter(|e| e.is_live())
            .cloned()
            .ok_or(CloudError::NotFound)?;
        let remote = entry.remote_id.clone().ok_or(CloudError::NotFound)?;
        if let Some(dir) = dest.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let part = dest.with_extension("part");
        let mut file = std::fs::File::create(&part).map_err(|e| CloudError::Permanent(e.to_string()))?;
        let (clip, account_id, size) = (job.clip, job.account.clone(), entry.size);
        let Worker { deferred, out, .. } = &mut *self;
        let out = &*out;
        let mut last = Instant::now();
        let result = provider.download(&remote, &mut file, &mut |done| {
            if last.elapsed() >= Duration::from_millis(400) {
                last = Instant::now();
                let fraction = (done as f64 / size.max(1) as f64) as f32;
                out(Update::Progress { clip, account: account_id.clone(), fraction, download: true });
            }
            deferred.extend(rx.try_iter());
            true
        });
        drop(file);
        if let Err(e) = result {
            let _ = std::fs::remove_file(&part);
            return Err(e);
        }
        std::fs::rename(&part, &dest).map_err(|e| CloudError::Permanent(e.to_string()))?;
        facts::after_download(self.meta.as_ref(), &dest, clip, &entry);
        log::info!("[Cloud] {clip} saved to {}", dest.display());
        (self.out)(Update::Downloaded);
        Ok(())
    }

    /// What a failure means for the job (spec §8).
    fn failed(&mut self, job: &Job, e: CloudError) {
        let quiet = matches!(e, CloudError::Cancelled);
        if !quiet {
            log::warn!("[Cloud] {:?} {} on {} failed: {e}", job.kind, job.clip, job.account);
        }
        match e {
            CloudError::SignIn => {
                self.queue.sign_in_needed(&job.account);
                self.providers.remove(&job.account);
                (self.out)(Update::AccountError(job.account.clone(), Some("Cloud_Error_SignIn")));
            }
            CloudError::Transient(_) => self.queue.fail(job.id, "Cloud_Error_Network", Utc::now()),
            CloudError::Cancelled => self.queue.requeue(job.id),
            CloudError::Full => self.queue.fail_permanently(job.id, "Cloud_Error_Full"),
            CloudError::OverLimit => self.queue.fail_permanently(job.id, "Cloud_Error_OverLimit"),
            // nothing left to delete or update: done
            CloudError::NotFound if matches!(job.kind, JobKind::Delete | JobKind::SyncMeta) => self.queue.complete(job.id),
            CloudError::NotFound => self.queue.fail_permanently(job.id, "Cloud_Error_NotFound"),
            CloudError::Permanent(_) => self.queue.fail_permanently(job.id, "Cloud_Error_Generic"),
        }
    }

    fn reload_index(&mut self, provider: &dyn CloudProvider, account: &AccountId) {
        match self.client(provider).load_index() {
            Ok((index, _)) => {
                self.store_index(account, index);
                self.publish_indexes();
            }
            Err(e) => log::warn!("[Cloud] reading the cloud library of {account} failed: {e}"),
        }
    }

    fn store_index(&mut self, account: &AccountId, index: CloudIndex) {
        let _ = std::fs::create_dir_all(&self.paths.dir);
        let _ = std::fs::write(self.paths.index(account), index.to_json());
        self.indexes.insert(account.clone(), index);
    }

    /// Reads every account's cloud index (other PCs change it too) and fetches missing thumbnails.
    fn refresh_indexes(&mut self) {
        self.last_refresh = Some(Instant::now());
        let accounts = self.settings.accounts.clone();
        for account in &accounts {
            let provider = match self.provider(account) {
                Ok(p) => p,
                Err(e) => {
                    let key = if e == CloudError::SignIn { "Cloud_Error_SignIn" } else { "Cloud_Error_Network" };
                    (self.out)(Update::AccountError(account.id.clone(), Some(key)));
                    continue;
                }
            };
            match self.client(provider.as_ref()).load_index() {
                Ok((index, _)) => {
                    self.store_index(&account.id, index);
                    (self.out)(Update::AccountError(account.id.clone(), None));
                }
                Err(e) => {
                    let key = if e == CloudError::SignIn { "Cloud_Error_SignIn" } else { "Cloud_Error_Network" };
                    log::warn!("[Cloud] reading the cloud library of {} failed: {e}", account.id);
                    (self.out)(Update::AccountError(account.id.clone(), Some(key)));
                }
            }
        }
        self.publish_indexes();
        self.fetch_thumbnails();
    }

    fn fetch_thumbnails(&mut self) {
        let mut wanted = Vec::new();
        for (account, index) in &self.indexes {
            for (id, e) in index.live() {
                let Some(thumb) = &e.thumbnail_id else { continue };
                let Some(target) = facts::thumbnail_path(&stream::key(*id, &e.file_name)) else { continue };
                if !target.exists() && wanted.len() < THUMBS_PER_REFRESH {
                    wanted.push((account.clone(), thumb.clone(), target));
                }
            }
        }
        if wanted.is_empty() {
            return;
        }
        let mut fetched = 0;
        for (account_id, thumb, target) in wanted {
            let Some(account) = self.account(&account_id).cloned() else { continue };
            let Ok(provider) = self.provider(&account) else { continue };
            let mut bytes = Vec::new();
            if provider.download(&thumb, &mut bytes, &mut |_| true).is_ok() {
                if let Some(dir) = target.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                if std::fs::write(&target, bytes).is_ok() {
                    fetched += 1;
                }
            }
        }
        if fetched > 0 {
            (self.out)(Update::ThumbnailsReady);
        }
    }

    fn publish_indexes(&self) {
        let mut list: Vec<(AccountId, CloudIndex)> = self
            .settings
            .accounts
            .iter()
            .filter_map(|a| self.indexes.get(&a.id).map(|i| (a.id.clone(), i.clone())))
            .collect();
        // primary first (the library takes a clip's facts from the first account that has it)
        list.sort_by_key(|(id, _)| self.account(id).map(|a| a.role as u8).unwrap_or(9));
        (self.out)(Update::Indexes(list));
    }

    fn publish_transfers(&self) {
        let mut map = HashMap::new();
        for j in self.queue.jobs().iter().filter(|j| matches!(j.kind, JobKind::Upload | JobKind::Download)) {
            let t = match &j.state {
                JobState::Pending | JobState::WaitingSignIn => Transfer::Queued,
                JobState::Running if j.kind == JobKind::Download => Transfer::Downloading(0.0),
                JobState::Running => Transfer::Uploading(0.0),
                JobState::Failed(reason) => Transfer::Failed(reason.clone()),
            };
            map.insert((j.clip, j.account.clone()), t);
        }
        (self.out)(Update::Transfers(map));
    }
}
