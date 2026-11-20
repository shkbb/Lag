//! The persistent transfer queue (spec §8): survives restarts, one job per (clip, account, kind), cheap jobs
//! first, retries with growing waits, an account whose sign-in expired waits for the user. The worker that
//! runs jobs and the providers build on this; nothing here touches the network.
use crate::ids::{AccountId, ClipId};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum JobKind {
    /// Metadata change (favorite, game) to write into the cloud index.
    SyncMeta,
    Delete,
    Upload,
    Download,
}

impl JobKind {
    /// Lower runs first: index updates and deletes take a moment, transfers take minutes.
    fn priority(self) -> u8 {
        match self {
            Self::SyncMeta => 0,
            Self::Delete => 1,
            Self::Upload => 2,
            Self::Download => 3,
        }
    }
}

/// A resumable upload in progress: the provider's session and how far it got.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UploadSession {
    pub url: String,
    pub offset: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum JobState {
    Pending,
    Running,
    /// Gave up after the retries (or a permanent error); the user can retry.
    Failed(String),
    /// The account must be signed in again.
    WaitingSignIn,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub id: u64,
    pub clip: ClipId,
    pub account: AccountId,
    pub kind: JobKind,
    /// Local file to upload, or where a download goes.
    pub local_path: Option<String>,
    pub session: Option<UploadSession>,
    pub attempts: u32,
    pub not_before: Option<DateTime<Utc>>,
    pub state: JobState,
    pub last_error: Option<String>,
}

/// Waits after the 1st, 2nd, ... failure; after the last one the job is Failed.
const BACKOFF_SECS: [i64; 4] = [30, 120, 600, 3600];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Queue {
    next_id: u64,
    jobs: Vec<Job>,
}

impl Queue {
    pub fn from_json(text: &str) -> serde_json::Result<Self> {
        let mut queue: Self = serde_json::from_str(text.trim_start_matches('\u{feff}'))?;
        // a job that was running when Lag stopped runs again (an upload resumes from its session)
        for job in &mut queue.jobs {
            if job.state == JobState::Running {
                job.state = JobState::Pending;
            }
        }
        Ok(queue)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("queue serializes")
    }

    pub fn jobs(&self) -> &[Job] {
        &self.jobs
    }

    pub fn get(&self, id: u64) -> Option<&Job> {
        self.jobs.iter().find(|j| j.id == id)
    }

    /// Adds work. A queued job of the same kind for the same clip and account is replaced (favorite toggled
    /// twice = one sync); a delete drops the queued uploads, downloads and syncs of that clip in that account.
    /// Running jobs are never touched (the worker finishes or cancels them). Returns the job id.
    pub fn push(&mut self, clip: ClipId, account: &AccountId, kind: JobKind, local_path: Option<String>) -> u64 {
        let waiting = |j: &Job| j.state != JobState::Running && j.clip == clip && &j.account == account;
        if kind == JobKind::Delete {
            self.jobs.retain(|j| !(waiting(j) && j.kind != JobKind::Delete));
        }
        if let Some(job) = self.jobs.iter_mut().find(|j| waiting(j) && j.kind == kind) {
            if job.local_path != local_path {
                job.session = None; // another file: the old upload session does not apply
            }
            job.local_path = local_path;
            job.attempts = 0;
            job.not_before = None;
            job.last_error = None;
            if !matches!(job.state, JobState::WaitingSignIn) {
                job.state = JobState::Pending;
            }
            return job.id;
        }
        self.next_id += 1;
        self.jobs.push(Job {
            id: self.next_id,
            clip,
            account: account.clone(),
            kind,
            local_path,
            session: None,
            attempts: 0,
            not_before: None,
            state: JobState::Pending,
            last_error: None,
        });
        self.next_id
    }

    /// The next job to run now: pending, its wait over, its account not blocked (paused by the upload rule,
    /// e.g. a game is running), cheap kinds first, then oldest first.
    pub fn next_ready(&self, now: DateTime<Utc>, blocked: impl Fn(&AccountId, JobKind) -> bool) -> Option<u64> {
        self.jobs
            .iter()
            .filter(|j| j.state == JobState::Pending)
            .filter(|j| j.not_before.is_none_or(|t| t <= now))
            .filter(|j| !blocked(&j.account, j.kind))
            .min_by_key(|j| (j.kind.priority(), j.id))
            .map(|j| j.id)
    }

    pub fn start(&mut self, id: u64) {
        if let Some(j) = self.job_mut(id) {
            j.state = JobState::Running;
        }
    }

    /// Progress of a resumable upload, kept so a restart continues from here.
    pub fn save_session(&mut self, id: u64, session: UploadSession) {
        if let Some(j) = self.job_mut(id) {
            j.session = Some(session);
        }
    }

    /// Done: the job leaves the queue.
    pub fn complete(&mut self, id: u64) {
        self.jobs.retain(|j| j.id != id);
    }

    /// A failure worth retrying (network, server busy): wait, then try again; Failed after the last wait.
    pub fn fail(&mut self, id: u64, reason: &str, now: DateTime<Utc>) {
        if let Some(j) = self.job_mut(id) {
            j.attempts += 1;
            j.last_error = Some(reason.to_owned());
            match BACKOFF_SECS.get(j.attempts as usize - 1) {
                Some(&secs) => {
                    j.state = JobState::Pending;
                    j.not_before = Some(now + Duration::seconds(secs));
                }
                None => j.state = JobState::Failed(reason.to_owned()),
            }
        }
    }

    /// A failure retrying cannot fix (file gone, cloud full, no permission).
    pub fn fail_permanently(&mut self, id: u64, reason: &str) {
        if let Some(j) = self.job_mut(id) {
            j.last_error = Some(reason.to_owned());
            j.state = JobState::Failed(reason.to_owned());
        }
    }

    /// The account's sign-in expired: every job of it waits (the running one included).
    pub fn sign_in_needed(&mut self, account: &AccountId) {
        for j in self.jobs.iter_mut().filter(|j| &j.account == account) {
            if !matches!(j.state, JobState::Failed(_)) {
                j.state = JobState::WaitingSignIn;
            }
        }
    }

    /// Signed in again: its jobs run.
    pub fn signed_in(&mut self, account: &AccountId) {
        for j in self.jobs.iter_mut().filter(|j| &j.account == account && j.state == JobState::WaitingSignIn) {
            j.state = JobState::Pending;
        }
    }

    /// "Retry" in the library.
    pub fn retry(&mut self, id: u64) {
        if let Some(j) = self.job_mut(id).filter(|j| matches!(j.state, JobState::Failed(_))) {
            j.state = JobState::Pending;
            j.attempts = 0;
            j.not_before = None;
        }
    }

    /// The account was disconnected: its jobs go.
    pub fn forget_account(&mut self, account: &AccountId) {
        self.jobs.retain(|j| &j.account != account);
    }

    fn job_mut(&mut self, id: u64) -> Option<&mut Job> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ProviderKind;
    use chrono::TimeZone;

    fn t(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_790_000_000 + secs, 0).unwrap()
    }
    fn drive() -> AccountId {
        AccountId::new(ProviderKind::GoogleDrive, "1")
    }
    const OPEN: fn(&AccountId, JobKind) -> bool = |_, _| false;

    #[test]
    fn duplicates_merge_and_a_delete_cancels_waiting_work() {
        let mut q = Queue::default();
        let clip = ClipId::new_random();
        let a = q.push(clip, &drive(), JobKind::SyncMeta, None);
        let b = q.push(clip, &drive(), JobKind::SyncMeta, None);
        assert_eq!(a, b, "favorite toggled twice = one sync");
        q.push(clip, &drive(), JobKind::Upload, Some("a.mp4".into()));
        q.push(clip, &drive(), JobKind::Delete, None);
        assert_eq!(q.jobs().iter().map(|j| j.kind).collect::<Vec<_>>(), [JobKind::Delete]);
        // the same clip in another account is untouched
        let dropbox = AccountId::new(ProviderKind::Dropbox, "2");
        q.push(clip, &dropbox, JobKind::Upload, Some("a.mp4".into()));
        q.push(clip, &drive(), JobKind::Delete, None);
        assert_eq!(q.jobs().len(), 2);
    }

    #[test]
    fn running_jobs_are_left_alone() {
        let mut q = Queue::default();
        let clip = ClipId::new_random();
        let up = q.push(clip, &drive(), JobKind::Upload, Some("a.mp4".into()));
        q.start(up);
        q.push(clip, &drive(), JobKind::Delete, None);
        assert_eq!(q.get(up).unwrap().state, JobState::Running, "the worker cancels it, not the queue");
        assert_eq!(q.jobs().len(), 2);
    }

    #[test]
    fn cheap_jobs_first_then_oldest() {
        let mut q = Queue::default();
        let up = q.push(ClipId::new_random(), &drive(), JobKind::Upload, Some("a".into()));
        let down = q.push(ClipId::new_random(), &drive(), JobKind::Download, Some("b".into()));
        let meta = q.push(ClipId::new_random(), &drive(), JobKind::SyncMeta, None);
        assert_eq!(q.next_ready(t(0), OPEN), Some(meta));
        q.complete(meta);
        assert_eq!(q.next_ready(t(0), OPEN), Some(up));
        // uploads paused while a game runs: downloads still go
        assert_eq!(q.next_ready(t(0), |_, k| k == JobKind::Upload), Some(down));
    }

    #[test]
    fn retries_wait_longer_each_time_then_fail() {
        let mut q = Queue::default();
        let id = q.push(ClipId::new_random(), &drive(), JobKind::Upload, Some("a".into()));
        for (i, wait) in BACKOFF_SECS.iter().enumerate() {
            q.start(id);
            q.fail(id, "network", t(0));
            assert_eq!(q.next_ready(t(wait - 1), OPEN), None, "attempt {i} still waiting");
            assert_eq!(q.next_ready(t(*wait), OPEN), Some(id));
        }
        q.fail(id, "network", t(0));
        assert_eq!(q.get(id).unwrap().state, JobState::Failed("network".into()));
        assert_eq!(q.next_ready(t(1_000_000), OPEN), None);
        q.retry(id);
        assert_eq!(q.next_ready(t(0), OPEN), Some(id));
    }

    #[test]
    fn expired_sign_in_pauses_the_account() {
        let mut q = Queue::default();
        let id = q.push(ClipId::new_random(), &drive(), JobKind::Upload, Some("a".into()));
        q.start(id);
        q.sign_in_needed(&drive());
        assert_eq!(q.next_ready(t(0), OPEN), None);
        q.signed_in(&drive());
        assert_eq!(q.next_ready(t(0), OPEN), Some(id));
        q.forget_account(&drive());
        assert!(q.jobs().is_empty());
    }

    #[test]
    fn survives_a_restart_mid_upload() {
        let mut q = Queue::default();
        let id = q.push(ClipId::new_random(), &drive(), JobKind::Upload, Some("a.mp4".into()));
        q.start(id);
        q.save_session(id, UploadSession { url: "https://upload/session".into(), offset: 8 << 20 });
        let restored = Queue::from_json(&q.to_json()).unwrap();
        let job = restored.get(id).unwrap();
        assert_eq!(job.state, JobState::Pending);
        assert_eq!(job.session.as_ref().unwrap().offset, 8 << 20, "continues where it stopped");
        // a new job after the restart gets a fresh id
        let mut restored = restored;
        assert!(restored.push(ClipId::new_random(), &drive(), JobKind::SyncMeta, None) > id);
    }
}
