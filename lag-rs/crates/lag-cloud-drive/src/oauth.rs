//! Sign-in for a desktop app (spec §9): OAuth 2 with PKCE and a loopback redirect, for every provider that
//! signs in this way (Google Drive, Dropbox, OneDrive). Lag opens the user's browser on the provider's page,
//! listens on the loopback for the redirect, trades the code for tokens and asks who signed in. A `Service`
//! describes one provider's addresses and rules. Nothing here stores anything (tokens.rs does).
use crate::http::{send_collect, HttpClient, Method, Request};
use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

pub const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
pub const REVOKE_URL: &str = "https://oauth2.googleapis.com/revoke";
pub const USERINFO_URL: &str = "https://openidconnect.googleapis.com/v1/userinfo";
pub const DRIVE_FILE_SCOPE: &str = "https://www.googleapis.com/auth/drive.file";
/// Only Lag's own files in Drive, and the e-mail to show which account is connected.
pub const SCOPES: &str = "openid email https://www.googleapis.com/auth/drive.file";

/// Who signed in: (stable user id, e-mail or name), from an access token.
pub type Identity = fn(&dyn HttpClient, &str) -> Result<(String, String), SignInError>;

/// One provider's sign-in.
pub struct Service {
    pub name: &'static str,
    pub auth_url: &'static str,
    pub token_url: &'static str,
    pub scopes: &'static str,
    /// Extra parameters of the authorize URL.
    pub extra: &'static [(&'static str, &'static str)],
    /// Loopback host of the redirect ("127.0.0.1" or "localhost").
    pub host: &'static str,
    /// A fixed port the app registration names (Dropbox registers the exact redirect), or any free port.
    pub port: Option<u16>,
    /// A scope the user must leave granted (Google lets them untick Drive).
    pub required_scope: Option<&'static str>,
    /// Who signed in: (stable user id, e-mail or name), from the access token.
    pub identity: Identity,
}

pub const GOOGLE: Service = Service {
    name: "Google",
    auth_url: AUTH_URL,
    token_url: TOKEN_URL,
    scopes: SCOPES,
    // a refresh token, every time (the user may have connected Lag before)
    extra: &[("access_type", "offline"), ("prompt", "consent")],
    host: "127.0.0.1",
    port: None,
    required_scope: Some(DRIVE_FILE_SCOPE),
    identity: google_identity,
};

/// Dropbox, "App folder" access: Lag sees only `Apps/Lag` in the user's Dropbox.
pub const DROPBOX: Service = Service {
    name: "Dropbox",
    auth_url: "https://www.dropbox.com/oauth2/authorize",
    token_url: "https://api.dropboxapi.com/oauth2/token",
    scopes: "account_info.read files.metadata.read files.metadata.write files.content.read files.content.write",
    extra: &[("token_access_type", "offline")],
    host: "localhost",
    port: Some(53_982),
    required_scope: None,
    identity: dropbox_identity,
};

/// OneDrive (personal Microsoft accounts), the app's own folder only (`Apps/Lag`).
pub const MICROSOFT: Service = Service {
    name: "Microsoft",
    auth_url: "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize",
    token_url: "https://login.microsoftonline.com/consumers/oauth2/v2.0/token",
    scopes: "offline_access User.Read Files.ReadWrite.AppFolder",
    extra: &[("prompt", "select_account")],
    host: "localhost",
    port: None,
    required_scope: None,
    identity: microsoft_identity,
};

/// Lag's OAuth client (from lag-rs/cloud-secrets.env at build time, build.rs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientConfig {
    pub id: String,
    /// Google issues one to desktop clients; Dropbox and Microsoft public clients have none (PKCE only).
    pub secret: String,
}

impl ClientConfig {
    pub fn built_in() -> Option<Self> {
        let id = option_env!("LAG_GOOGLE_CLIENT_ID")?;
        let secret = option_env!("LAG_GOOGLE_CLIENT_SECRET").unwrap_or("");
        (!id.is_empty()).then(|| Self { id: id.to_owned(), secret: secret.to_owned() })
    }
    pub fn dropbox() -> Option<Self> {
        let id = option_env!("LAG_DROPBOX_APP_KEY")?;
        (!id.is_empty()).then(|| Self { id: id.to_owned(), secret: String::new() })
    }
    pub fn onedrive() -> Option<Self> {
        let id = option_env!("LAG_ONEDRIVE_CLIENT_ID")?;
        (!id.is_empty()).then(|| Self { id: id.to_owned(), secret: String::new() })
    }
}

/// What a signed-in account keeps (encrypted by tokens.rs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
    /// The provider's stable user id (the account id in Lag).
    pub sub: String,
    pub email: String,
}

impl Tokens {
    /// Refresh a minute early so a request never starts with a token about to expire.
    pub fn expired(&self, now: DateTime<Utc>) -> bool {
        now + Duration::seconds(60) >= self.expires_at
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignInError {
    /// The build has no OAuth client (cloud-secrets.env missing).
    NotConfigured,
    /// The user pressed "Cancel" on the provider's page, or closed the flow in Lag.
    Cancelled,
    /// The user signed in but unticked the storage access Lag needs.
    DriveNotGranted,
    TimedOut,
    /// The refresh token was revoked or expired: sign in again.
    Revoked,
    Network(String),
    /// The provider refused (the name keeps its first use).
    Google(String),
}

impl std::fmt::Display for SignInError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConfigured => f.write_str("sign-in is not configured in this build"),
            Self::Cancelled => f.write_str("sign-in cancelled"),
            Self::DriveNotGranted => f.write_str("access to the storage was not granted"),
            Self::TimedOut => f.write_str("sign-in timed out"),
            Self::Revoked => f.write_str("sign-in expired, sign in again"),
            Self::Network(e) => write!(f, "network error: {e}"),
            Self::Google(e) => write!(f, "the provider refused: {e}"),
        }
    }
}

impl std::error::Error for SignInError {}

pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

impl Pkce {
    pub fn from_verifier(verifier: String) -> Self {
        let challenge = b64url(&crate::win::sha256(verifier.as_bytes()));
        Self { verifier, challenge }
    }
    pub fn new() -> Self {
        Self::from_verifier(b64url(&crate::win::random_bytes::<32>()))
    }
}

impl Default for Pkce {
    fn default() -> Self {
        Self::new()
    }
}

pub fn auth_url_for(service: &Service, client: &ClientConfig, redirect_uri: &str, pkce: &Pkce, state: &str) -> String {
    let mut pairs: Vec<(&str, &str)> = vec![
        ("client_id", &client.id),
        ("redirect_uri", redirect_uri),
        ("response_type", "code"),
        ("scope", service.scopes),
        ("code_challenge", &pkce.challenge),
        ("code_challenge_method", "S256"),
        ("state", state),
    ];
    pairs.extend(service.extra.iter().copied());
    format!("{}{}", service.auth_url, crate::http::query(&pairs))
}

/// Google's authorize URL.
pub fn auth_url(client: &ClientConfig, redirect_uri: &str, pkce: &Pkce, state: &str) -> String {
    auth_url_for(&GOOGLE, client, redirect_uri, pkce, state)
}

/// What the browser brought back to the loopback listener.
#[derive(Debug, PartialEq, Eq)]
pub enum Callback {
    Code(String),
    Denied,
    /// Not the redirect (favicon, a stray request): answer 404 and keep listening.
    Other,
}

/// Parses `GET /?state=..&code=.. HTTP/1.1`; a wrong state is a forged redirect and is ignored.
pub fn parse_callback(request_line: &str, state: &str) -> Callback {
    let Some(target) = request_line.split_whitespace().nth(1) else { return Callback::Other };
    let Some((_, query)) = target.split_once('?') else { return Callback::Other };
    let pairs: Vec<(String, String)> =
        form_urlencoded::parse(query.as_bytes()).map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    let get = |key: &str| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
    if get("state") != Some(state) {
        return Callback::Other;
    }
    match (get("code"), get("error")) {
        (Some(code), _) if !code.is_empty() => Callback::Code(code.to_owned()),
        (_, Some(_)) => Callback::Denied,
        _ => Callback::Other,
    }
}

#[derive(Deserialize)]
struct TokenAnswer {
    access_token: String,
    expires_in: i64,
    refresh_token: Option<String>,
    scope: Option<String>,
}

fn provider_error(body: &[u8]) -> String {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            let e = v.get("error")?;
            Some(match e.as_str() {
                Some(code) => format!("{code}: {}", v.get("error_description").and_then(|d| d.as_str()).unwrap_or("")),
                None => e.get("message").and_then(|m| m.as_str()).unwrap_or("unknown error").to_owned(),
            })
        })
        .unwrap_or_else(|| String::from_utf8_lossy(body).chars().take(200).collect())
}

fn net(e: std::io::Error) -> SignInError {
    SignInError::Network(e.to_string())
}

fn json_get(http: &dyn HttpClient, request: Request) -> Result<serde_json::Value, SignInError> {
    let (response, body) = send_collect(http, request).map_err(net)?;
    if !response.ok() {
        return Err(SignInError::Google(provider_error(&body)));
    }
    serde_json::from_slice(&body).map_err(|e| SignInError::Google(e.to_string()))
}

fn google_identity(http: &dyn HttpClient, access: &str) -> Result<(String, String), SignInError> {
    let v = json_get(http, Request::new(Method::Get, USERINFO_URL).bearer(access))?;
    let sub = v["sub"].as_str().ok_or_else(|| SignInError::Google("no user id".into()))?.to_owned();
    Ok((sub, v["email"].as_str().unwrap_or_default().to_owned()))
}

fn dropbox_identity(http: &dyn HttpClient, access: &str) -> Result<(String, String), SignInError> {
    let request = Request::new(Method::Post, "https://api.dropboxapi.com/2/users/get_current_account")
        .bearer(access)
        .bytes("application/json", b"null".to_vec());
    let v = json_get(http, request)?;
    let id = v["account_id"].as_str().ok_or_else(|| SignInError::Google("no account id".into()))?.to_owned();
    Ok((id, v["email"].as_str().unwrap_or_default().to_owned()))
}

fn microsoft_identity(http: &dyn HttpClient, access: &str) -> Result<(String, String), SignInError> {
    let v = json_get(http, Request::new(Method::Get, "https://graph.microsoft.com/v1.0/me").bearer(access))?;
    let id = v["id"].as_str().ok_or_else(|| SignInError::Google("no user id".into()))?.to_owned();
    let name = v["mail"].as_str().or_else(|| v["userPrincipalName"].as_str()).unwrap_or_default().to_owned();
    Ok((id, name))
}

/// The client's id (+ secret, when the provider issued one) on a token request.
fn client_pairs<'a>(client: &'a ClientConfig, pairs: &mut Vec<(&'a str, &'a str)>) {
    pairs.push(("client_id", &client.id));
    if !client.secret.is_empty() {
        pairs.push(("client_secret", &client.secret));
    }
}

/// Trades the browser's code for tokens and finds out who signed in.
pub fn exchange_code_for(
    service: &Service,
    http: &dyn HttpClient,
    client: &ClientConfig,
    code: &str,
    redirect_uri: &str,
    pkce: &Pkce,
) -> Result<Tokens, SignInError> {
    let mut pairs = vec![
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("grant_type", "authorization_code"),
        ("code_verifier", pkce.verifier.as_str()),
    ];
    client_pairs(client, &mut pairs);
    let (response, body) = send_collect(http, Request::new(Method::Post, service.token_url).form(&pairs)).map_err(net)?;
    if !response.ok() {
        return Err(SignInError::Google(provider_error(&body)));
    }
    let answer: TokenAnswer = serde_json::from_slice(&body).map_err(|e| SignInError::Google(e.to_string()))?;
    if let Some(required) = service.required_scope {
        if !answer.scope.as_deref().unwrap_or("").split(' ').any(|s| s == required) {
            return Err(SignInError::DriveNotGranted);
        }
    }
    let refresh_token = answer.refresh_token.ok_or_else(|| SignInError::Google("no refresh token".into()))?;
    let (sub, email) = (service.identity)(http, &answer.access_token)?;
    Ok(Tokens {
        access_token: answer.access_token,
        refresh_token,
        expires_at: Utc::now() + Duration::seconds(answer.expires_in),
        sub,
        email,
    })
}

/// Google's code exchange.
pub fn exchange_code(
    http: &dyn HttpClient,
    client: &ClientConfig,
    code: &str,
    redirect_uri: &str,
    pkce: &Pkce,
) -> Result<Tokens, SignInError> {
    exchange_code_for(&GOOGLE, http, client, code, redirect_uri, pkce)
}

/// A new access token from the refresh token. `Revoked` = the user must sign in again.
pub fn refresh_for(
    service: &Service,
    http: &dyn HttpClient,
    client: &ClientConfig,
    tokens: &mut Tokens,
) -> Result<(), SignInError> {
    let mut pairs = vec![("refresh_token", tokens.refresh_token.as_str()), ("grant_type", "refresh_token")];
    client_pairs(client, &mut pairs);
    let (response, body) = send_collect(http, Request::new(Method::Post, service.token_url).form(&pairs)).map_err(net)?;
    if response.status == 400 || response.status == 401 {
        let error = provider_error(&body);
        if error.starts_with("invalid_grant") || response.status == 401 {
            return Err(SignInError::Revoked);
        }
        return Err(SignInError::Google(error));
    }
    if !response.ok() {
        return Err(SignInError::Network(format!("HTTP {}", response.status)));
    }
    let answer: TokenAnswer = serde_json::from_slice(&body).map_err(|e| SignInError::Google(e.to_string()))?;
    tokens.access_token = answer.access_token;
    tokens.expires_at = Utc::now() + Duration::seconds(answer.expires_in);
    if let Some(rotated) = answer.refresh_token {
        tokens.refresh_token = rotated;
    }
    Ok(())
}

/// Google's refresh.
pub fn refresh(http: &dyn HttpClient, client: &ClientConfig, tokens: &mut Tokens) -> Result<(), SignInError> {
    refresh_for(&GOOGLE, http, client, tokens)
}

/// "Disconnect" in Settings: the provider forgets Lag's access too (best effort; Microsoft has no such call for
/// personal accounts: the user removes it at account.live.com/consent/Manage).
pub fn revoke_for(service: &Service, http: &dyn HttpClient, tokens: &Tokens) {
    let request = match service.name {
        "Google" => Request::new(Method::Post, REVOKE_URL).form(&[("token", &tokens.refresh_token)]),
        "Dropbox" => {
            Request::new(Method::Post, "https://api.dropboxapi.com/2/auth/token/revoke").bearer(&tokens.access_token)
        }
        _ => return,
    };
    if let Err(e) = send_collect(http, request) {
        log::warn!("[Cloud] revoking the {} token failed: {e}", service.name);
    }
}

/// Google's revoke.
pub fn revoke(http: &dyn HttpClient, tokens: &Tokens) {
    revoke_for(&GOOGLE, http, tokens)
}

/// The page the browser shows after the redirect.
const DONE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Lag</title>\
<body style=\"font-family:Segoe UI,sans-serif;background:#16121f;color:#eee;display:grid;place-items:center;height:90vh\">\
<div style=\"text-align:center\"><h2>Lag</h2><p>{MSG}</p></div>";

/// Listeners for the redirect: "localhost" may reach the IPv6 loopback first, so both are listened on.
fn listen(service: &Service) -> Result<(Vec<TcpListener>, u16), SignInError> {
    let first = TcpListener::bind(("127.0.0.1", service.port.unwrap_or(0))).map_err(net)?;
    let port = first.local_addr().map_err(net)?.port();
    let mut listeners = vec![first];
    if service.host == "localhost" {
        if let Ok(v6) = TcpListener::bind(("::1", port)) {
            listeners.push(v6);
        }
    }
    for l in &listeners {
        l.set_nonblocking(true).map_err(net)?;
    }
    Ok((listeners, port))
}

/// The whole sign-in: listen on the loopback, open the provider's page with `open_browser`, wait for the
/// redirect (until `timeout`, or `cancel` is set), exchange the code.
pub fn sign_in_for(
    service: &Service,
    http: &dyn HttpClient,
    client: &ClientConfig,
    open_browser: &dyn Fn(&str) -> std::io::Result<()>,
    timeout: std::time::Duration,
    cancel: &AtomicBool,
) -> Result<Tokens, SignInError> {
    let (listeners, port) = listen(service)?;
    let redirect_uri = format!("http://{}:{port}", service.host);
    let pkce = Pkce::new();
    let state = b64url(&crate::win::random_bytes::<16>());
    open_browser(&auth_url_for(service, client, &redirect_uri, &pkce, &state)).map_err(net)?;

    let started = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(SignInError::Cancelled);
        }
        if started.elapsed() > timeout {
            return Err(SignInError::TimedOut);
        }
        let accepted = listeners.iter().find_map(|l| match l.accept() {
            Ok((stream, _)) => Some(Ok(stream)),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => None,
            Err(e) => Some(Err(e)),
        });
        let mut stream = match accepted {
            Some(Ok(stream)) => stream,
            Some(Err(e)) => return Err(net(e)),
            None => {
                std::thread::sleep(std::time::Duration::from_millis(100));
                continue;
            }
        };
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
        let mut line = String::new();
        let _ = BufReader::new(&stream).read_line(&mut line);
        let callback = parse_callback(&line, &state);
        let connected = format!("{} is connected to Lag. You can close this tab and go back to Lag.", service.name);
        let (status, message) = match &callback {
            Callback::Code(_) => ("200 OK", connected.as_str()),
            Callback::Denied => ("200 OK", "Sign-in was cancelled. You can close this tab."),
            Callback::Other => ("404 Not Found", "Not found."),
        };
        let page = DONE_PAGE.replace("{MSG}", message);
        let _ = write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
            page.len()
        );
        match callback {
            Callback::Code(code) => return exchange_code_for(service, http, client, &code, &redirect_uri, &pkce),
            Callback::Denied => return Err(SignInError::Cancelled),
            Callback::Other => continue,
        }
    }
}

/// Google's sign-in.
pub fn sign_in(
    http: &dyn HttpClient,
    client: &ClientConfig,
    open_browser: &dyn Fn(&str) -> std::io::Result<()>,
    timeout: std::time::Duration,
    cancel: &AtomicBool,
) -> Result<Tokens, SignInError> {
    sign_in_for(&GOOGLE, http, client, open_browser, timeout, cancel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::fake::FakeHttp;
    use std::io::Read;

    fn client() -> ClientConfig {
        ClientConfig { id: "cid.apps.googleusercontent.com".into(), secret: "shh".into() }
    }

    #[test]
    fn pkce_follows_rfc_7636() {
        // the RFC's own example
        let p = Pkce::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into());
        assert_eq!(p.challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        let fresh = Pkce::new();
        assert_eq!(fresh.verifier.len(), 43);
        assert_ne!(fresh.verifier, Pkce::new().verifier);
    }

    #[test]
    fn the_auth_url_asks_for_exactly_what_lag_needs() {
        let url = auth_url(&client(), "http://127.0.0.1:5000", &Pkce::from_verifier("v".repeat(43)), "st");
        assert!(url.starts_with(AUTH_URL));
        for part in ["code_challenge_method=S256", "access_type=offline", "prompt=consent", "state=st", "drive%2Efile"] {
            assert!(url.contains(part), "{part} in {url}");
        }
        assert!(!url.contains("shh"), "the secret never goes to the browser");
    }

    #[test]
    fn callbacks_are_checked() {
        assert_eq!(parse_callback("GET /?state=st&code=4%2Fabc&scope=x HTTP/1.1", "st"), Callback::Code("4/abc".into()));
        assert_eq!(parse_callback("GET /?error=access_denied&state=st HTTP/1.1", "st"), Callback::Denied);
        assert_eq!(parse_callback("GET /?state=forged&code=x HTTP/1.1", "st"), Callback::Other);
        assert_eq!(parse_callback("GET /favicon.ico HTTP/1.1", "st"), Callback::Other);
        assert_eq!(parse_callback("", "st"), Callback::Other);
    }

    fn token_answer(scope: &str) -> Vec<u8> {
        serde_json::json!({"access_token": "at", "expires_in": 3599, "refresh_token": "rt", "scope": scope})
            .to_string()
            .into_bytes()
    }

    #[test]
    fn sign_in_end_to_end_through_the_loopback() {
        let http = FakeHttp::default();
        http.expect(|req| {
            let body = String::from_utf8_lossy(&req.body).into_owned();
            assert!(body.contains("grant_type=authorization_code") && body.contains("code=4%2Fabc"), "{body}");
            assert!(body.contains("code_verifier="));
            (200, vec![], token_answer(&format!("openid {DRIVE_FILE_SCOPE} email")))
        });
        http.expect(|req| {
            assert_eq!(req.header("Authorization"), Some("Bearer at"));
            (200, vec![], br#"{"sub":"1234","email":"me@example.com"}"#.to_vec())
        });
        // the "browser": follows the redirect to the loopback with the state from the URL
        let browser = |url: &str| -> std::io::Result<()> {
            let query = url.split_once('?').unwrap().1;
            let get = |key: &str| {
                form_urlencoded::parse(query.as_bytes()).find(|(k, _)| k == key).map(|(_, v)| v.into_owned()).unwrap()
            };
            let (redirect, state) = (get("redirect_uri"), get("state"));
            std::thread::spawn(move || {
                let addr = redirect.trim_start_matches("http://");
                let mut s = std::net::TcpStream::connect(addr).unwrap();
                write!(s, "GET /favicon.ico HTTP/1.1\r\n\r\n").unwrap();
                let mut s = std::net::TcpStream::connect(addr).unwrap();
                write!(s, "GET /?state={state}&code=4%2Fabc HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
                let mut page = String::new();
                let _ = s.read_to_string(&mut page);
                assert!(page.contains("connected"), "{page}");
            });
            Ok(())
        };
        let tokens = sign_in(&http, &client(), &browser, std::time::Duration::from_secs(10), &AtomicBool::new(false))
            .unwrap();
        assert_eq!((tokens.sub.as_str(), tokens.email.as_str(), tokens.refresh_token.as_str()), ("1234", "me@example.com", "rt"));
        assert!(!tokens.expired(Utc::now()));
        assert_eq!(http.remaining(), 0);
    }

    #[test]
    fn unticked_drive_access_and_cancel_are_reported() {
        let http = FakeHttp::default();
        http.expect(|_| (200, vec![], token_answer("openid email")));
        let err = exchange_code(&http, &client(), "c", "http://127.0.0.1:1", &Pkce::new()).unwrap_err();
        assert_eq!(err, SignInError::DriveNotGranted);
        let cancelled = sign_in(&http, &client(), &|_| Ok(()), std::time::Duration::from_secs(5), &AtomicBool::new(true));
        assert_eq!(cancelled.unwrap_err(), SignInError::Cancelled);
    }

    #[test]
    fn refresh_renews_or_reports_revoked() {
        let mut tokens = Tokens {
            access_token: "old".into(),
            refresh_token: "rt".into(),
            expires_at: Utc::now(),
            sub: "1".into(),
            email: String::new(),
        };
        let http = FakeHttp::default();
        http.expect(|req| {
            assert!(String::from_utf8_lossy(&req.body).contains("grant_type=refresh_token"));
            (200, vec![], br#"{"access_token":"new","expires_in":3600}"#.to_vec())
        });
        refresh(&http, &client(), &mut tokens).unwrap();
        assert_eq!((tokens.access_token.as_str(), tokens.refresh_token.as_str()), ("new", "rt"));
        http.expect(|_| (400, vec![], br#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#.to_vec()));
        assert_eq!(refresh(&http, &client(), &mut tokens).unwrap_err(), SignInError::Revoked);
    }

    #[test]
    fn public_clients_never_send_a_secret_and_ask_their_own_identity() {
        let public = ClientConfig { id: "app-key".into(), secret: String::new() };
        let url = auth_url_for(&DROPBOX, &public, "http://localhost:53982", &Pkce::from_verifier("v".repeat(43)), "st");
        assert!(url.starts_with(DROPBOX.auth_url) && url.contains("token_access_type=offline"), "{url}");
        assert!(!url.contains("access_type=offline&prompt"), "no Google parameters");

        let http = FakeHttp::default();
        http.expect(|req| {
            let body = String::from_utf8_lossy(&req.body).into_owned();
            assert!(req.url.starts_with("https://api.dropboxapi.com/oauth2/token"));
            assert!(!body.contains("client_secret"), "PKCE public client: {body}");
            (200, vec![], br#"{"access_token":"at","expires_in":14400,"refresh_token":"rt"}"#.to_vec())
        });
        http.expect(|req| {
            assert!(req.url.ends_with("/2/users/get_current_account"));
            assert_eq!(req.header("Authorization"), Some("Bearer at"));
            (200, vec![], br#"{"account_id":"dbid:42","email":"me@example.com"}"#.to_vec())
        });
        let tokens = exchange_code_for(&DROPBOX, &http, &public, "c", "http://localhost:53982", &Pkce::new()).unwrap();
        assert_eq!((tokens.sub.as_str(), tokens.email.as_str()), ("dbid:42", "me@example.com"));

        http.expect(|_| (200, vec![], br#"{"access_token":"at2","expires_in":3600,"refresh_token":"rt2","scope":"x"}"#.to_vec()));
        http.expect(|req| {
            assert!(req.url.ends_with("/v1.0/me"));
            (200, vec![], br#"{"id":"ms-7","userPrincipalName":"me@outlook.com"}"#.to_vec())
        });
        let tokens = exchange_code_for(&MICROSOFT, &http, &public, "c", "http://localhost:1", &Pkce::new()).unwrap();
        assert_eq!((tokens.sub.as_str(), tokens.email.as_str()), ("ms-7", "me@outlook.com"));
        assert_eq!(http.remaining(), 0);
    }
}
