//! Opens the storage behind an account: the one place that knows which provider is which. Everything above
//! (the worker, usage, streaming) talks to `lag_cloud::provider::CloudProvider` only.
use lag_cloud::provider::{CloudError, CloudProvider};
use lag_cloud::settings::AccountSettings;
use lag_cloud::ProviderKind;
use lag_cloud_drive::tokens::TokenStore;
use lag_cloud_drive::winhttp::WinHttp;
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
        other => Err(CloudError::Permanent(format!("{other:?} is not supported yet"))),
    }
}
