//! Where a signed-in account's tokens live (spec §9): `%LOCALAPPDATA%\Lag\cloud\<account>.token`, encrypted
//! with DPAPI for the current Windows user. Never in settings.json, never in logs.
use crate::oauth::Tokens;
use lag_cloud::AccountId;
use std::io;
use std::path::{Path, PathBuf};

pub struct TokenStore {
    dir: PathBuf,
}

impl TokenStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn for_user() -> Option<Self> {
        std::env::var_os("LOCALAPPDATA").map(|p| Self::new(Path::new(&p).join("Lag").join("cloud")))
    }

    fn path(&self, account: &AccountId) -> PathBuf {
        let name: String =
            account.as_str().chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
        self.dir.join(format!("{name}.token"))
    }

    pub fn save(&self, account: &AccountId, tokens: &Tokens) -> io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let json = serde_json::to_vec(tokens).map_err(io::Error::other)?;
        let sealed = crate::win::protect(&json)?;
        // write-then-rename: a crash never leaves half a token file
        let path = self.path(account);
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, sealed)?;
        std::fs::rename(&tmp, &path)
    }

    /// None if the account was never signed in here (or the file cannot be decrypted, e.g. copied from
    /// another PC or Windows user: sign in again).
    pub fn load(&self, account: &AccountId) -> Option<Tokens> {
        let sealed = std::fs::read(self.path(account)).ok()?;
        match crate::win::unprotect(&sealed).ok().and_then(|json| serde_json::from_slice(&json).ok()) {
            Some(tokens) => Some(tokens),
            None => {
                log::warn!("[Cloud] stored sign-in for {account} cannot be read; sign in again");
                None
            }
        }
    }

    pub fn forget(&self, account: &AccountId) {
        let _ = std::fs::remove_file(self.path(account));
        let _ = std::fs::remove_file(self.path(account).with_extension("secret"));
    }

    /// A password (WebDAV app password), encrypted the same way, next to where tokens would be.
    pub fn save_secret(&self, account: &AccountId, secret: &str) -> io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let sealed = crate::win::protect(secret.as_bytes())?;
        let path = self.path(account).with_extension("secret");
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, sealed)?;
        std::fs::rename(&tmp, &path)
    }

    pub fn load_secret(&self, account: &AccountId) -> Option<String> {
        let sealed = std::fs::read(self.path(account).with_extension("secret")).ok()?;
        String::from_utf8(crate::win::unprotect(&sealed).ok()?).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use lag_cloud::ProviderKind;

    #[test]
    fn tokens_are_stored_encrypted_and_come_back() {
        let dir = std::env::temp_dir().join(format!("lag-token-test-{}", std::process::id()));
        let store = TokenStore::new(&dir);
        let account = AccountId::new(ProviderKind::GoogleDrive, "1234");
        let tokens = Tokens {
            access_token: "at".into(),
            refresh_token: "very-secret-refresh-token".into(),
            expires_at: Utc::now(),
            sub: "1234".into(),
            email: "me@example.com".into(),
        };
        store.save(&account, &tokens).unwrap();
        let raw = std::fs::read(dir.join("gdrive_1234.token")).unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains("very-secret"), "never in plain text");
        assert_eq!(store.load(&account), Some(tokens));
        std::fs::write(dir.join("gdrive_1234.token"), b"garbage").unwrap();
        assert_eq!(store.load(&account), None, "unreadable = sign in again");
        store.save_secret(&account, "app-password").unwrap();
        assert_eq!(store.load_secret(&account).as_deref(), Some("app-password"));
        assert!(!String::from_utf8_lossy(&std::fs::read(dir.join("gdrive_1234.secret")).unwrap()).contains("app-password"));
        store.forget(&account);
        assert_eq!(store.load(&account), None);
        assert_eq!(store.load_secret(&account), None, "disconnecting forgets the password too");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
