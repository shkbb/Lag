//! The cloud as the app sees it (docs/spec/cloud.md). UI thread only (`Rc`); network work happens on the cloud
//! worker thread (`worker`) and on short-lived sign-in / usage threads, and comes back through the event loop.
//! * `mod.rs`    settings (`%APPDATA%\Lag\cloud.json`), accounts, sign-in, space; the cloud's state for the
//!               library (indexes + transfers) and the commands the library sends (upload, delete, download);
//! * `worker`    the background thread that runs the transfer queue and keeps the index copies;
//! * `providers` which storage an account is (Google Drive, ...);
//! * `facts`     what the worker reads / writes about a clip on this PC (metadata, thumbnails, dates);
//! * `stream`    cloud-only clips as keys (`cloud:<id>/<name>`) and playing them from the cloud.
//!
//! Offline until `go_online` (the real app calls it; --snapshot / --gpu-check / tests never touch the network,
//! the user's cloud or their files): the page still shows the stored settings and accounts.
mod facts;
mod providers;
pub mod stream;
mod worker;

pub(crate) use facts::thumbnail_path;

use lag_cloud::index::CloudIndex;
use lag_cloud::presence::Transfer;
use lag_cloud::settings::{AccountSettings, CloudSettings, Role};
use lag_cloud::sync::{AccountClient, Usage};
use lag_cloud::{AccountId, ClipId, ProviderKind};
use lag_cloud_drive::tokens::TokenStore;
use lag_cloud_drive::winhttp::WinHttp;
use lag_cloud_drive::{oauth, win, ClientConfig, SignInError, Tokens};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;
use worker::{Cmd, Update};

/// How long Lag waits for the user to finish signing in in the browser.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);

/// One connected account as the settings page shows it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AccountView {
    pub settings: AccountSettings,
    /// None while unknown (offline, or still asking).
    pub usage: Option<Usage>,
    /// i18n key of the last problem with this account.
    pub error: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CloudView {
    pub settings: CloudSettings,
    pub accounts: Vec<AccountView>,
    /// A sign-in is open in the browser.
    pub connecting: bool,
    /// i18n key of the last sign-in problem.
    pub error: Option<&'static str>,
    /// This build has an OAuth client (lag-rs/cloud-secrets.env at build time).
    pub configured: bool,
}

/// What the library needs from the cloud: every account's index (primary first) and the transfers.
#[derive(Debug, Clone, Default)]
pub struct CloudLibrary {
    pub indexes: Vec<(AccountId, CloudIndex)>,
    pub transfers: HashMap<(ClipId, AccountId), Transfer>,
    /// A cloud is connected (the library offers the cloud actions).
    pub connected: bool,
}

/// Why the library is told.
#[derive(Debug, Clone)]
pub enum LibraryEvent {
    /// Indexes or transfers changed: re-join the cloud with the local clips.
    Changed,
    /// A local clip got its cloud id.
    IdAssigned { path: String, id: ClipId },
    /// Files changed on disk (a download finished, thumbnails arrived): rescan.
    Rescan,
}

thread_local! { static SERVICE: RefCell<Weak<CloudService>> = const { RefCell::new(Weak::new()) }; }

/// Per account: its space (once known) and the i18n key of its last problem.
type AccountStatus = (Option<Usage>, Option<&'static str>);
type ViewListener = Rc<dyn Fn(&CloudView)>;
type LibraryListener = Rc<dyn Fn(&LibraryEvent)>;

/// Runs `f` on the UI thread with the service (if the app still has it).
fn post(f: impl FnOnce(&Rc<CloudService>) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(service) = SERVICE.with(|s| s.borrow().upgrade()) {
            f(&service);
        }
    });
}

pub struct CloudService {
    path: Option<PathBuf>,
    settings: RefCell<CloudSettings>,
    status: RefCell<HashMap<AccountId, AccountStatus>>,
    connecting: RefCell<Option<Arc<AtomicBool>>>,
    error: Cell<Option<&'static str>>,
    online: Cell<bool>,
    listeners: RefCell<Vec<ViewListener>>,
    worker: RefCell<Option<Sender<Cmd>>>,
    library: RefCell<CloudLibrary>,
    library_listeners: RefCell<Vec<LibraryListener>>,
}

impl CloudService {
    /// The stored cloud settings (nothing in tests: they never read the user's files).
    pub fn load() -> Rc<Self> {
        let path = if cfg!(test) {
            None
        } else {
            std::env::var_os("APPDATA").map(|p| PathBuf::from(p).join("Lag").join("cloud.json"))
        };
        let settings = path.as_deref().map(CloudSettings::load).unwrap_or_default();
        let service = Rc::new(Self {
            path,
            settings: RefCell::new(settings),
            status: RefCell::default(),
            connecting: RefCell::default(),
            error: Cell::new(None),
            online: Cell::new(false),
            listeners: RefCell::default(),
            worker: RefCell::default(),
            library: RefCell::default(),
            library_listeners: RefCell::default(),
        });
        SERVICE.with(|s| *s.borrow_mut() = Rc::downgrade(&service));
        service
    }

    /// The real app: network allowed from now on; the worker starts once a cloud is connected.
    pub fn go_online(self: &Rc<Self>) {
        self.online.set(true);
        self.sync_worker();
        self.refresh_usage();
    }

    pub fn view(&self) -> CloudView {
        let settings = self.settings.borrow().clone();
        let status = self.status.borrow();
        CloudView {
            accounts: settings
                .accounts
                .iter()
                .map(|a| {
                    let (usage, error) = status.get(&a.id).cloned().unwrap_or_default();
                    AccountView { settings: a.clone(), usage, error }
                })
                .collect(),
            settings,
            connecting: self.connecting.borrow().is_some(),
            error: self.error.get(),
            configured: ClientConfig::built_in().is_some(),
        }
    }

    pub fn on_changed(&self, f: impl Fn(&CloudView) + 'static) {
        self.listeners.borrow_mut().push(Rc::new(f));
    }

    fn notify(&self) {
        let view = self.view();
        let listeners: Vec<_> = self.listeners.borrow().clone();
        for f in listeners {
            f(&view);
        }
    }

    /// Only the real app writes cloud.json (--snapshot / --gpu-check run with the user's APPDATA).
    fn persist(&self) {
        if !self.online.get() {
            return;
        }
        if let Some(path) = &self.path {
            if let Err(e) = self.settings.borrow().save(path) {
                log::warn!("[Cloud] saving cloud.json failed: {e}");
            }
        }
    }

    /// Starts the worker when a cloud is connected, and tells it the current settings.
    fn sync_worker(&self) {
        if !self.online.get() {
            return;
        }
        let settings = self.settings.borrow().clone();
        let has_accounts = settings.enabled && !settings.accounts.is_empty();
        if !has_accounts {
            self.library.borrow_mut().connected = false;
        }
        let mut worker = self.worker.borrow_mut();
        match worker.as_ref() {
            Some(tx) => {
                let _ = tx.send(Cmd::Settings(settings));
            }
            None if has_accounts => {
                let Some(paths) = worker::Paths::for_user() else { return };
                *worker = Some(worker::spawn(settings, paths, Box::new(|update| post(move |s| s.on_update(update)))));
            }
            None => {}
        }
    }

    fn send(&self, cmd: Cmd) {
        if let Some(tx) = self.worker.borrow().as_ref() {
            let _ = tx.send(cmd);
        }
    }

    /// A settings change from the page.
    pub fn edit(&self, f: impl FnOnce(&mut CloudSettings)) {
        let changed = {
            let mut s = self.settings.borrow_mut();
            let before = s.clone();
            f(&mut s);
            s.normalize();
            *s != before
        };
        if changed {
            self.persist();
            self.sync_worker();
            self.notify();
        }
    }

    // ---- what the library and the app ask -------------------------------------------------------------------

    pub fn library(&self) -> CloudLibrary {
        self.library.borrow().clone()
    }

    pub fn on_library(&self, f: impl Fn(&LibraryEvent) + 'static) {
        self.library_listeners.borrow_mut().push(Rc::new(f));
    }

    fn tell_library(&self, event: LibraryEvent) {
        let listeners: Vec<_> = self.library_listeners.borrow().clone();
        for f in listeners {
            f(&event);
        }
    }

    /// A replay or screenshot was saved: the upload rule decides whether it goes up.
    pub fn clip_saved(&self, path: &str) {
        self.send(Cmd::ClipSaved(path.to_owned()));
    }

    /// "Upload to the cloud" on a clip.
    pub fn upload_now(&self, path: &str) {
        self.send(Cmd::UploadNow(path.to_owned()));
    }

    pub fn favorite_changed(&self, path: Option<&str>, id: Option<ClipId>, favorite: bool) {
        self.send(Cmd::Favorite { path: path.map(str::to_owned), id, favorite });
    }

    pub fn delete_from_cloud(&self, id: ClipId) {
        self.send(Cmd::DeleteFromCloud(id));
    }

    /// "Save to this PC".
    pub fn download(&self, id: ClipId, dir: PathBuf) {
        self.send(Cmd::Download { id, dir });
    }

    pub fn retry(&self) {
        self.send(Cmd::Retry);
    }

    /// Settings > Library opened: the space and the cloud libraries are read again.
    pub fn refresh(&self) {
        self.send(Cmd::Refresh);
        self.refresh_usage();
    }

    /// The cloud's web page for a clip ("Open in Google Drive").
    pub fn web_link(&self, id: ClipId) -> Option<String> {
        let library = self.library.borrow();
        let (account, entry) = library.indexes.iter().find_map(|(a, i)| i.entries.get(&id).filter(|e| e.is_live()).map(|e| (a, e)))?;
        let settings = self.settings.borrow();
        let account = settings.accounts.iter().find(|a| &a.id == account)?;
        match account.provider {
            ProviderKind::GoogleDrive => Some(format!("https://drive.google.com/file/d/{}/view", entry.remote_id.as_deref()?)),
            _ => None,
        }
    }

    fn on_update(&self, update: Update) {
        match update {
            Update::Indexes(list) => {
                let settings = self.settings.borrow().clone();
                let mut directory = HashMap::new();
                for (account, index) in &list {
                    let Some(acc) = settings.accounts.iter().find(|a| &a.id == account) else { continue };
                    for (id, e) in index.live() {
                        if let Some(remote) = &e.remote_id {
                            directory.entry(*id).or_insert_with(|| (acc.clone(), remote.clone()));
                        }
                    }
                }
                stream::set_directory(directory);
                {
                    let mut lib = self.library.borrow_mut();
                    lib.indexes = list;
                    lib.connected = !settings.accounts.is_empty();
                }
                self.tell_library(LibraryEvent::Changed);
            }
            Update::Transfers(map) => {
                self.library.borrow_mut().transfers = map;
                self.tell_library(LibraryEvent::Changed);
            }
            Update::Progress { clip, account, fraction, download } => {
                let t = if download { Transfer::Downloading(fraction) } else { Transfer::Uploading(fraction) };
                self.library.borrow_mut().transfers.insert((clip, account), t);
                self.tell_library(LibraryEvent::Changed);
            }
            Update::AccountError(account, key) => {
                let mut status = self.status.borrow_mut();
                let entry = status.entry(account).or_default();
                if entry.1 != key {
                    entry.1 = key;
                    drop(status);
                    self.notify();
                }
            }
            Update::IdAssigned { path, id } => self.tell_library(LibraryEvent::IdAssigned { path, id }),
            Update::Downloaded | Update::ThumbnailsReady => self.tell_library(LibraryEvent::Rescan),
        }
    }

    // ---- accounts ---------------------------------------------------------------------------------------

    /// "Connect": opens Google's sign-in in the browser and waits for it on a thread.
    pub fn connect(&self) {
        if self.connecting.borrow().is_some() {
            return;
        }
        let Some(client) = ClientConfig::built_in() else {
            self.error.set(Some("Cloud_Error_NotConfigured"));
            self.notify();
            return;
        };
        if !self.online.get() {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        *self.connecting.borrow_mut() = Some(cancel.clone());
        self.error.set(None);
        self.notify();
        std::thread::spawn(move || {
            let result = WinHttp::new()
                .map_err(|e| SignInError::Network(e.to_string()))
                .and_then(|http| oauth::sign_in(&http, &client, &win::open_browser, SIGN_IN_TIMEOUT, &cancel));
            post(move |s| s.finish_connect(result));
        });
    }

    pub fn cancel_connect(&self) {
        if let Some(cancel) = self.connecting.borrow().as_ref() {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    fn finish_connect(&self, result: Result<Tokens, SignInError>) {
        *self.connecting.borrow_mut() = None;
        match result {
            Ok(tokens) => {
                let id = AccountId::new(ProviderKind::GoogleDrive, &tokens.sub);
                if let Some(store) = TokenStore::for_user() {
                    if let Err(e) = store.save(&id, &tokens) {
                        log::warn!("[Cloud] storing the sign-in failed: {e}");
                    }
                }
                log::info!("[Cloud] Google Drive connected ({id})");
                self.add_account(AccountSettings {
                    id: id.clone(),
                    provider: ProviderKind::GoogleDrive,
                    display_name: tokens.email.clone(),
                    ..Default::default()
                });
                self.send(Cmd::SignedIn(id));
            }
            Err(SignInError::Cancelled) => self.error.set(None),
            Err(e) => {
                log::warn!("[Cloud] sign-in failed: {e}");
                self.error.set(Some(match e {
                    SignInError::DriveNotGranted => "Cloud_Error_NoDrive",
                    SignInError::TimedOut => "Cloud_Error_Timeout",
                    SignInError::Network(_) => "Cloud_Error_Network",
                    SignInError::NotConfigured => "Cloud_Error_NotConfigured",
                    _ => "Cloud_Error_Generic",
                }));
            }
        }
        self.notify();
    }

    /// A newly connected account (or the same one signed in again): the first one is the primary.
    fn add_account(&self, account: AccountSettings) {
        {
            let mut s = self.settings.borrow_mut();
            let first = s.accounts.is_empty();
            match s.accounts.iter_mut().find(|a| a.id == account.id) {
                Some(a) => a.display_name = account.display_name.clone(),
                None => s.accounts.push(AccountSettings {
                    role: if first { Role::Primary } else { Role::Mirror },
                    ..account.clone()
                }),
            }
            s.enabled = true;
            s.normalize();
        }
        self.status.borrow_mut().remove(&account.id);
        self.error.set(None);
        self.persist();
        self.sync_worker();
        self.refresh_usage();
    }

    /// "Disconnect": forgets the account here and tells the provider to forget Lag's access. Files stay in the
    /// cloud.
    pub fn disconnect(&self, id: &AccountId) {
        if let Some(store) = TokenStore::for_user() {
            if let (Some(tokens), true) = (store.load(id), self.online.get()) {
                std::thread::spawn(move || {
                    if let Ok(http) = WinHttp::new() {
                        oauth::revoke(&http, &tokens);
                    }
                });
            }
            store.forget(id);
        }
        {
            let mut s = self.settings.borrow_mut();
            s.accounts.retain(|a| &a.id != id);
            if s.accounts.is_empty() {
                s.enabled = false;
            }
            s.normalize();
        }
        self.status.borrow_mut().remove(id);
        log::info!("[Cloud] {id} disconnected");
        self.persist();
        self.sync_worker();
        self.notify();
    }

    /// --ui-check only: shows a connected account with its space, in memory (nothing stored, no network), so
    /// the connected layout of the Library tab can be checked without signing in.
    pub fn preview_connected(&self) {
        let id = AccountId::new(ProviderKind::GoogleDrive, "preview");
        {
            let mut s = self.settings.borrow_mut();
            s.enabled = true;
            s.accounts = vec![AccountSettings {
                id: id.clone(),
                display_name: "player@example.com".into(),
                ..Default::default()
            }];
        }
        let space = lag_cloud::quota::ProviderSpace { free: 4_800 << 30, total: Some(5_120 << 30) };
        self.status.borrow_mut().insert(id, (Some(Usage { lag_bytes: 37 << 30, clips: 214, space }), None));
        self.notify();
    }

    /// Asks every connected account how much space it has (Settings shows it).
    pub fn refresh_usage(&self) {
        if !self.online.get() {
            return;
        }
        let settings = self.settings.borrow().clone();
        for account in settings.accounts.clone() {
            let (root, sort) = (settings.root_folder.clone(), settings.sort);
            std::thread::spawn(move || {
                let result = providers::open(&account).and_then(|provider| {
                    AccountClient { provider: provider.as_ref(), root, sort, device: device_name() }.usage()
                });
                let id = account.id.clone();
                post(move |s| {
                    let entry = match result {
                        Ok(usage) => (Some(usage), None),
                        Err(lag_cloud::provider::CloudError::SignIn) => (None, Some("Cloud_Error_SignIn")),
                        Err(_) => (None, Some("Cloud_Error_Network")),
                    };
                    s.status.borrow_mut().insert(id, entry);
                    s.notify();
                });
            });
        }
    }
}

/// Local clips the library's size limit must treat with care (paths lowercased): `in_cloud` are safely in a
/// cloud (their local copy can go first, losing nothing), `busy` are moving right now (never touched).
#[derive(Debug, Clone, Default)]
pub struct LocalGuard {
    pub in_cloud: std::collections::HashSet<String>,
    pub busy: std::collections::HashSet<String>,
}

static GUARD: std::sync::LazyLock<std::sync::Mutex<LocalGuard>> = std::sync::LazyLock::new(Default::default);

/// Set by the library after joining the cloud; read by the size limit on its own thread.
pub fn set_local_guard(guard: LocalGuard) {
    *GUARD.lock().unwrap() = guard;
}

pub fn local_guard() -> LocalGuard {
    GUARD.lock().unwrap().clone()
}

/// `CloudService::clip_saved` on the app's service, for code that has no context at hand (the screenshot thread
/// comes back through the event loop with only the path).
pub fn clip_saved(path: &str) {
    if let Some(service) = SERVICE.with(|s| s.borrow().upgrade()) {
        service.clip_saved(path);
    }
}

/// `CloudService::preview_connected` on the app's service (for --ui-check scripts).
pub fn preview_connected() {
    if let Some(service) = SERVICE.with(|s| s.borrow().upgrade()) {
        service.preview_connected();
    }
}

fn device_name() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "PC".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_service_shows_settings_and_never_signs_in() {
        let service = CloudService::load();
        let seen = Rc::new(Cell::new(0));
        let s2 = seen.clone();
        service.on_changed(move |_| s2.set(s2.get() + 1));
        // offline (tests, --snapshot): connect does nothing that reaches the network
        service.connect();
        assert!(!service.view().connecting);
        service.edit(|s| s.upload_screenshots = false);
        assert!(!service.view().settings.upload_screenshots);
        service.edit(|s| s.upload_screenshots = false);
        assert_eq!(seen.get(), 1 + usize::from(!service.view().configured), "an unchanged edit notifies nobody");
        // offline: no worker, so library commands go nowhere (and nothing panics)
        service.clip_saved(r"D:\Lag\a.mp4");
        assert!(!service.library().connected);
    }
}
