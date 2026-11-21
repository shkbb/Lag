//! Opens the storage behind an account: the one place that knows which provider is which. Everything above
//! (the worker, usage, streaming) talks to `lag_cloud::provider::CloudProvider` only.
use lag_cloud::provider::{CloudError, CloudProvider};
use lag_cloud::settings::AccountSettings;
use lag_cloud::ProviderKind;
use lag_cloud_drive::tokens::TokenStore;
use lag_cloud_drive::winhttp::WinHttp;
use lag_cloud_drive::oauth::{self, Service};
use lag_cloud_drive::session::OAuthSession;
use lag_cloud_drive::{ClientConfig, GoogleDrive};
use std::sync::Arc;

/// The account's storage, signed in with what is stored on this PC. `SignIn` = connect it again.
pub(crate) fn open(account: &AccountSettings) -> Result<Arc<dyn CloudProvider>, CloudError> {
    match account.provider {
        ProviderKind::GoogleDrive => {
            let client = ClientConfig::built_in()
                .ok_or_else(|| CloudError::Permanent("Google sign-in is not configured in this build".into()))?;
            let store = TokenStore::for_user().ok_or(CloudError::SignIn)?;
            let tokens = store.load(&account.id).ok_or(CloudError::SignIn)?;
            let http = Arc::new(WinHttp::new().map_err(|e| CloudError::Transient(e.to_string()))?);
            let id = account.id.clone();
            Ok(Arc::new(GoogleDrive::new(http, client, tokens, move |t| {
                if let Err(e) = store.save(&id, t) {
                    log::warn!("[Cloud] storing a refreshed sign-in failed: {e}");
                }
            })))
        }
        ProviderKind::Folder => {
            if account.location.trim().is_empty() {
                return Err(CloudError::Permanent("no folder".into()));
            }
            Ok(Arc::new(lag_cloud_drive::folder::FolderStorage::new(account.location.trim())))
        }
        ProviderKind::WebDav => {
            let store = TokenStore::for_user().ok_or(CloudError::SignIn)?;
            let password = store.load_secret(&account.id).ok_or(CloudError::SignIn)?;
            let http = Arc::new(WinHttp::new().map_err(|e| CloudError::Transient(e.to_string()))?);
            Ok(Arc::new(lag_cloud_drive::webdav::WebDav::new(http, &account.location, &account.user, &password)))
        }
        ProviderKind::Dropbox | ProviderKind::OneDrive => {
            let (service, client) = oauth_of(account.provider)
                .ok_or_else(|| CloudError::Permanent(format!("{:?} sign-in is not configured in this build", account.provider)))?;
            let store = TokenStore::for_user().ok_or(CloudError::SignIn)?;
            let tokens = store.load(&account.id).ok_or(CloudError::SignIn)?;
            let http = Arc::new(WinHttp::new().map_err(|e| CloudError::Transient(e.to_string()))?);
            let id = account.id.clone();
            let session = OAuthSession::new(http, service, client, tokens, move |t| {
                if let Err(e) = store.save(&id, t) {
                    log::warn!("[Cloud] storing a refreshed sign-in failed: {e}");
                }
            });
            Ok(if account.provider == ProviderKind::Dropbox {
                Arc::new(lag_cloud_drive::dropbox::Dropbox::new(session))
            } else {
                Arc::new(lag_cloud_drive::onedrive::OneDrive::new(session))
            })
        }
    }
}

/// The browser sign-in of a provider and this build's client for it (None: no such sign-in, or no client).
pub(crate) fn oauth_of(kind: ProviderKind) -> Option<(&'static Service, ClientConfig)> {
    match kind {
        ProviderKind::GoogleDrive => ClientConfig::built_in().map(|c| (&oauth::GOOGLE, c)),
        ProviderKind::Dropbox => ClientConfig::dropbox().map(|c| (&oauth::DROPBOX, c)),
        ProviderKind::OneDrive => ClientConfig::onedrive().map(|c| (&oauth::MICROSOFT, c)),
        ProviderKind::Folder | ProviderKind::WebDav => None,
    }
}
