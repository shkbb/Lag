//! The cloud as the app sees it (docs/spec/cloud.md): the user's cloud settings (`%APPDATA%\Lag\cloud.json`),
//! the connected accounts with their space, and connecting / disconnecting Google Drive. UI thread only (`Rc`);
//! every network call runs on its own thread and comes back through the event loop.
//!
//! Offline until `go_online` (the real app calls it; --snapshot / --gpu-check / tests never touch the network
//! or the user's Google account): the page still shows the stored settings and accounts.
use lag_cloud::settings::{AccountSettings, CloudSettings, Role};
use lag_cloud::sync::{AccountClient, Usage};
use lag_cloud::{AccountId, ProviderKind};
use lag_cloud_drive::tokens::TokenStore;
use lag_cloud_drive::winhttp::WinHttp;
use lag_cloud_drive::{oauth, win, ClientConfig, GoogleDrive, SignInError, Tokens};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

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

thread_local! { static SERVICE: RefCell<Weak<CloudService>> = const { RefCell::new(Weak::new()) }; }

/// Per account: its space (once known) and the i18n key of its last problem.
type AccountStatus = (Option<Usage>, Option<&'static str>);
type ViewListener = Rc<dyn Fn(&CloudView)>;

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
        });
        SERVICE.with(|s| *s.borrow_mut() = Rc::downgrade(&service));
        service
    }

    /// The real app: network allowed from now on.
    pub fn go_online(self: &Rc<Self>) {
        self.online.set(true);
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
            self.notify();
        }
    }

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
                let email = tokens.email.clone();
                {
                    let mut s = self.settings.borrow_mut();
                    let first = s.accounts.is_empty();
                    match s.accounts.iter_mut().find(|a| a.id == id) {
                        Some(a) => a.display_name = email,
                        None => s.accounts.push(AccountSettings {
                            id: id.clone(),
                            provider: ProviderKind::GoogleDrive,
                            display_name: email,
                            role: if first { Role::Primary } else { Role::Mirror },
                            ..Default::default()
                        }),
                    }
                    s.enabled = true;
                    s.normalize();
                }
                self.status.borrow_mut().remove(&id);
                self.error.set(None);
                self.persist();
                self.refresh_usage();
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

    /// "Disconnect": forgets the account here and tells Google to forget Lag's access. Files stay in the cloud.
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
        let Some(client) = ClientConfig::built_in() else { return };
        let settings = self.settings.borrow().clone();
        for account in settings.accounts.iter().filter(|a| a.provider == ProviderKind::GoogleDrive) {
            let (id, client, root, sort) =
                (account.id.clone(), client.clone(), settings.root_folder.clone(), settings.sort);
            std::thread::spawn(move || {
                let result = drive_for(&id, client).and_then(|drive| {
                    let pc = AccountClient { provider: &drive, root, sort, device: device_name() };
                    pc.usage().map_err(|e| match e {
                        lag_cloud::provider::CloudError::SignIn => "Cloud_Error_SignIn",
                        _ => "Cloud_Error_Network",
                    })
                });
                post(move |s| {
                    let entry = match result {
                        Ok(usage) => (Some(usage), None),
                        Err(key) => (None, Some(key)),
                    };
                    s.status.borrow_mut().insert(id, entry);
                    s.notify();
                });
            });
        }
    }
}

/// `CloudService::preview_connected` on the app's service (for --ui-check scripts).
pub fn preview_connected() {
    if let Some(service) = SERVICE.with(|s| s.borrow().upgrade()) {
        service.preview_connected();
    }
}

/// The account's Drive from its stored sign-in (refreshed tokens are stored again).
fn drive_for(id: &AccountId, client: ClientConfig) -> Result<GoogleDrive, &'static str> {
    let store = TokenStore::for_user().ok_or("Cloud_Error_SignIn")?;
    let tokens = store.load(id).ok_or("Cloud_Error_SignIn")?;
    let http = Arc::new(WinHttp::new().map_err(|_| "Cloud_Error_Network")?);
    let saver = TokenStore::for_user().ok_or("Cloud_Error_SignIn")?;
    let id_for_save = id.clone();
    Ok(GoogleDrive::new(http, client, tokens, move |t| {
        if let Err(e) = saver.save(&id_for_save, t) {
            log::warn!("[Cloud] storing a refreshed sign-in failed: {e}");
        }
    }))
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
    }
}
