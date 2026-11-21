//! A signed-in OAuth session shared by the token-based providers (Dropbox, OneDrive): a fresh access token for
//! every request (refreshed a minute before it expires, and once more on a 401), refreshed tokens handed to the
//! token store, a revoked sign-in reported as `SignIn`.
use crate::http::{HttpClient, Request, Response};
use crate::oauth::{self, ClientConfig, Service, SignInError, Tokens};
use chrono::Utc;
use lag_cloud::provider::{CloudError, CloudResult};
use std::io::Write;
use std::sync::{Arc, Mutex};

pub struct OAuthSession {
    pub http: Arc<dyn HttpClient>,
    service: &'static Service,
    client: ClientConfig,
    tokens: Mutex<Tokens>,
    on_tokens: Box<dyn Fn(&Tokens) + Send + Sync>,
}

pub fn transport(e: std::io::Error) -> CloudError {
    if e.kind() == std::io::ErrorKind::Interrupted {
        CloudError::Cancelled
    } else {
        CloudError::Transient(e.to_string())
    }
}

impl OAuthSession {
    pub fn new(
        http: Arc<dyn HttpClient>,
        service: &'static Service,
        client: ClientConfig,
        tokens: Tokens,
        on_tokens: impl Fn(&Tokens) + Send + Sync + 'static,
    ) -> Self {
        Self { http, service, client, tokens: Mutex::new(tokens), on_tokens: Box::new(on_tokens) }
    }

    pub fn tokens(&self) -> Tokens {
        self.tokens.lock().unwrap().clone()
    }

    /// A valid access token, refreshed first if it is about to expire (or `force`).
    pub fn token(&self, force: bool) -> CloudResult<String> {
        let mut tokens = self.tokens.lock().unwrap();
        if force || tokens.expired(Utc::now()) {
            oauth::refresh_for(self.service, self.http.as_ref(), &self.client, &mut tokens).map_err(|e| match e {
                SignInError::Revoked => CloudError::SignIn,
                SignInError::Network(e) => CloudError::Transient(e),
                other => CloudError::Permanent(other.to_string()),
            })?;
            (self.on_tokens)(&tokens);
        }
        Ok(tokens.access_token.clone())
    }

    /// Sends a request built fresh for each try (bodies are consumed), signed with the access token; refreshes
    /// the token once on a 401.
    pub fn call(&self, build: &dyn Fn(&str) -> Request<'static>, sink: &mut dyn Write) -> CloudResult<Response> {
        let mut force = false;
        loop {
            let token = self.token(force)?;
            let response = self.http.send(build(&token), sink).map_err(transport)?;
            if response.status == 401 && !force {
                force = true;
                continue;
            }
            return Ok(response);
        }
    }
}

/// JSON with every non-ASCII character escaped (`\uXXXX`): Dropbox's `Dropbox-API-Arg` header takes ASCII only,
/// and a game name in Cyrillic must survive it.
pub fn ascii_json(value: &serde_json::Value) -> String {
    let mut out = String::new();
    for c in value.to_string().chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            let mut units = [0u16; 2];
            for u in c.encode_utf16(&mut units) {
                out.push_str(&format!("\\u{:04x}", u));
            }
        }
    }
    out
}

/// Counts bytes into the sink and stops when the progress callback says so.
pub struct Counting<'a> {
    pub inner: &'a mut dyn Write,
    pub done: u64,
    pub progress: lag_cloud::provider::Progress<'a>,
}

impl Write for Counting<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.done += n as u64;
        if !(self.progress)(self.done) {
            return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled"));
        }
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn non_ascii_is_escaped_for_headers() {
        let v = serde_json::json!({"path": "/Lag/Сталкер 2/😀.mp4"});
        let s = ascii_json(&v);
        assert!(s.is_ascii(), "{s}");
        assert_eq!(serde_json::from_str::<serde_json::Value>(&s).unwrap(), v, "same JSON once parsed");
    }
}
