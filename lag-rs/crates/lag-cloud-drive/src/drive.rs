//! Google Drive (REST v3) behind `CloudProvider`. Scope `drive.file`: Lag sees and touches only files it
//! created, so listing "everything" is listing Lag's own files.
//! * folders are found by name under their parent (and cached), created when missing;
//! * small files (the index) carry Drive's `version` as their revision: written only if it did not change;
//! * videos go up as resumable uploads in 8 MB chunks; a resumed upload asks Google how far it got;
//! * access tokens are refreshed before they expire and once more on a 401; a revoked sign-in is `SignIn`.
use crate::http::{query, HttpClient, Method, Request, Response};
use crate::oauth::{self, ClientConfig, SignInError, Tokens};
use chrono::Utc;
use lag_cloud::provider::*;
use lag_cloud::queue::UploadSession;
use lag_cloud::quota::ProviderSpace;
use lag_cloud::{AccountId, ProviderKind};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{self, SeekFrom, Write};
use std::sync::{Arc, Mutex};

const API: &str = "https://www.googleapis.com/drive/v3";
const UPLOAD_API: &str = "https://www.googleapis.com/upload/drive/v3";
const FOLDER_MIME: &str = "application/vnd.google-apps.folder";
const FILE_FIELDS: &str = "id,name,size,sha256Checksum,appProperties";
/// Resumable chunks must be multiples of 256 KiB; 8 MB keeps progress smooth and retries cheap.
pub const CHUNK: u64 = 32 * 256 * 1024;

pub struct GoogleDrive {
    http: Arc<dyn HttpClient>,
    client: ClientConfig,
    tokens: Mutex<Tokens>,
    /// Called after a refresh, so the new access token is stored (tokens.rs).
    on_tokens: Box<dyn Fn(&Tokens) + Send + Sync>,
    folders: Mutex<HashMap<Vec<String>, String>>,
    chunk: u64,
}

impl GoogleDrive {
    pub fn new(
        http: Arc<dyn HttpClient>,
        client: ClientConfig,
        tokens: Tokens,
        on_tokens: impl Fn(&Tokens) + Send + Sync + 'static,
    ) -> Self {
        Self {
            http,
            client,
            tokens: Mutex::new(tokens),
            on_tokens: Box::new(on_tokens),
            folders: Mutex::new(HashMap::new()),
            chunk: CHUNK,
        }
    }

    pub fn account_id(&self) -> AccountId {
        AccountId::new(ProviderKind::GoogleDrive, &self.tokens.lock().unwrap().sub)
    }

    pub fn email(&self) -> String {
        self.tokens.lock().unwrap().email.clone()
    }

    #[cfg(test)]
    fn with_chunk(mut self, chunk: u64) -> Self {
        self.chunk = chunk;
        self
    }

    /// A valid access token, refreshed first if it is about to expire (or `force`).
    fn token(&self, force: bool) -> CloudResult<String> {
        let mut tokens = self.tokens.lock().unwrap();
        if force || tokens.expired(Utc::now()) {
            oauth::refresh(self.http.as_ref(), &self.client, &mut tokens).map_err(|e| match e {
                SignInError::Revoked => CloudError::SignIn,
                SignInError::Network(e) => CloudError::Transient(e),
                other => CloudError::Permanent(other.to_string()),
            })?;
            (self.on_tokens)(&tokens);
        }
        Ok(tokens.access_token.clone())
    }

    /// Sends a request built fresh for each try (bodies are consumed): refreshes the token once on a 401.
    fn call(&self, build: &dyn Fn(&str) -> Request<'static>, sink: &mut dyn Write) -> CloudResult<Response> {
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

    /// `call` for small JSON answers; a non-2xx answer becomes the matching `CloudError`.
    fn call_json(&self, build: &dyn Fn(&str) -> Request<'static>) -> CloudResult<Value> {
        let mut body = Vec::new();
        let response = self.call(build, &mut body)?;
        if !response.ok() {
            return Err(status_error(&response));
        }
        if body.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&body).map_err(|e| CloudError::Permanent(format!("bad answer from Drive: {e}")))
    }

    /// The id of the child named `name` of `parent` (a folder when `folder`), if any.
    fn find_child(&self, parent: &str, name: &str, folder: bool) -> CloudResult<Option<(String, String)>> {
        let mime = if folder { format!(" and mimeType = '{FOLDER_MIME}'") } else { format!(" and mimeType != '{FOLDER_MIME}'") };
        let q = format!("name = '{}' and '{}' in parents and trashed = false{mime}", escape(name), escape(parent));
        let url = format!("{API}/files{}", query(&[("q", &q), ("fields", "files(id,version)"), ("spaces", "drive")]));
        let answer = self.call_json(&|t| Request::new(Method::Get, url.clone()).bearer(t))?;
        Ok(answer["files"].as_array().and_then(|f| f.first()).map(|f| {
            (f["id"].as_str().unwrap_or_default().to_owned(), f["version"].as_str().unwrap_or_default().to_owned())
        }))
    }

    /// The folder at `path`; created with its parents when `create`, else None if any part is missing.
    fn folder(&self, path: &[String], create: bool) -> CloudResult<Option<String>> {
        let mut parent = "root".to_owned();
        for depth in 1..=path.len() {
            let key = path[..depth].to_vec();
            if let Some(id) = self.folders.lock().unwrap().get(&key) {
                parent = id.clone();
                continue;
            }
            let name = &path[depth - 1];
            let id = match self.find_child(&parent, name, true)? {
                Some((id, _)) => id,
                None if create => {
                    let meta = json!({"name": name, "mimeType": FOLDER_MIME, "parents": [parent]});
                    let url = format!("{API}/files?fields=id");
                    let created = self.call_json(&|t| Request::new(Method::Post, url.clone()).bearer(t).json(&meta))?;
                    created["id"].as_str().unwrap_or_default().to_owned()
                }
                None => return Ok(None),
            };
            self.folders.lock().unwrap().insert(key, id.clone());
            parent = id;
        }
        Ok(Some(parent))
    }

    /// Asks Google how much of a resumable upload it has. Ok(Err(file)) = it is already complete.
    fn upload_status(&self, url: &str, size: u64) -> CloudResult<Result<u64, RemoteFile>> {
        let mut body = Vec::new();
        let range = format!("bytes */{size}");
        let response = self.call(
            &|t| Request::new(Method::Put, url.to_owned()).bearer(t).header("Content-Range", range.clone()).no_redirects(),
            &mut body,
        )?;
        match response.status {
            308 => Ok(Ok(confirmed(&response))),
            200 | 201 => Ok(Err(remote_file(&parse(&body)?))),
            404 | 410 => Err(CloudError::NotFound),
            _ => Err(status_error(&response)),
        }
    }

    fn start_upload(&self, folder_id: &str, name: &str, size: u64, props: &FileProps) -> CloudResult<String> {
        let meta = json!({"name": name, "parents": [folder_id], "appProperties": props});
        let url = format!("{UPLOAD_API}/files{}", query(&[("uploadType", "resumable"), ("fields", FILE_FIELDS)]));
        let mut body = Vec::new();
        let content_type = mime_of(name).to_owned();
        let response = self.call(
            &|t| {
                Request::new(Method::Post, url.clone())
                    .bearer(t)
                    .header("X-Upload-Content-Type", content_type.clone())
                    .header("X-Upload-Content-Length", size.to_string())
                    .json(&meta)
            },
            &mut body,
        )?;
        if !response.ok() {
            return Err(status_error(&response));
        }
        response
            .header("location")
            .map(str::to_owned)
            .ok_or_else(|| CloudError::Permanent("Drive gave no upload session".into()))
    }
}

fn transport(e: io::Error) -> CloudError {
    if e.kind() == io::ErrorKind::Interrupted {
        CloudError::Cancelled
    } else {
        CloudError::Transient(e.to_string())
    }
}

/// Drive's query string literal.
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

fn mime_of(name: &str) -> &'static str {
    match name.rsplit('.').next().map(str::to_ascii_lowercase).as_deref() {
        Some("mp4") => "video/mp4",
        Some("mkv") => "video/x-matroska",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    }
}

fn parse(body: &[u8]) -> CloudResult<Value> {
    serde_json::from_slice(body).map_err(|e| CloudError::Permanent(format!("bad answer from Drive: {e}")))
}

fn remote_file(v: &Value) -> RemoteFile {
    RemoteFile {
        id: v["id"].as_str().unwrap_or_default().to_owned(),
        name: v["name"].as_str().unwrap_or_default().to_owned(),
        size: v["size"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0),
        hash: v["sha256Checksum"].as_str().map(str::to_owned),
        props: v["appProperties"]
            .as_object()
            .map(|o| o.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned()))).collect())
            .unwrap_or_default(),
    }
}

/// Bytes Google confirmed, from a 308's `Range: bytes=0-N` (none = 0).
fn confirmed(response: &Response) -> u64 {
    response
        .header("range")
        .and_then(|r| r.rsplit('-').next())
        .and_then(|n| n.trim().parse::<u64>().ok())
        .map_or(0, |last| last + 1)
}

/// Drive's error answer as a `CloudError` (spec §8: what is retried, what waits for the user).
fn status_error(response: &Response) -> CloudError {
    let v: Value = serde_json::from_slice(&response.error_body).unwrap_or(Value::Null);
    let reason = v["error"]["errors"][0]["reason"].as_str().unwrap_or("");
    let message = v["error"]["message"].as_str().unwrap_or("").to_owned();
    match response.status {
        401 => CloudError::SignIn,
        403 if reason == "storageQuotaExceeded" || reason == "quotaExceeded" => CloudError::Full,
        403 if reason.contains("ateLimitExceeded") => CloudError::Transient(message),
        404 => CloudError::NotFound,
        408 | 429 | 500..=599 => CloudError::Transient(format!("HTTP {} {message}", response.status)),
        s => CloudError::Permanent(format!("HTTP {s} {reason} {message}")),
    }
}

/// Counts bytes into the sink and stops when the progress callback says so.
struct Counting<'a> {
    inner: &'a mut dyn Write,
    done: u64,
    progress: Progress<'a>,
}

impl Write for Counting<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.done += n as u64;
        if !(self.progress)(self.done) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl CloudProvider for GoogleDrive {
    fn ensure_folder(&self, path: &[String]) -> CloudResult<String> {
        Ok(self.folder(path, true)?.expect("created"))
    }

    fn read_small(&self, path: &[String]) -> CloudResult<Option<(Vec<u8>, Revision)>> {
        let Some((name, dir)) = path.split_last() else { return Ok(None) };
        let Some(folder) = self.folder(dir, false)? else { return Ok(None) };
        let Some((id, version)) = self.find_child(&folder, name, false)? else { return Ok(None) };
        let url = format!("{API}/files/{id}?alt=media");
        let mut body = Vec::new();
        let response = self.call(&|t| Request::new(Method::Get, url.clone()).bearer(t), &mut body)?;
        if !response.ok() {
            return Err(status_error(&response));
        }
        Ok(Some((body, Revision(version))))
    }

    fn write_small(&self, path: &[String], bytes: &[u8], expected: Option<&Revision>) -> CloudResult<WriteOutcome> {
        let (name, dir) = path.split_last().ok_or_else(|| CloudError::Permanent("empty path".into()))?;
        let folder = self.ensure_folder(dir)?;
        let current = self.find_child(&folder, name, false)?;
        if current.as_ref().map(|(_, v)| Revision(v.clone())).as_ref() != expected {
            return Ok(WriteOutcome::Conflict);
        }
        let written = match current {
            Some((id, _)) => {
                let url = format!("{UPLOAD_API}/files/{id}{}", query(&[("uploadType", "media"), ("fields", "id,version")]));
                self.call_json(&|t| Request::new(Method::Patch, url.clone()).bearer(t).bytes(mime_of(name), bytes.to_vec()))?
            }
            None => {
                let boundary = format!("lag{}", Utc::now().timestamp_nanos_opt().unwrap_or_default());
                let meta = json!({"name": name, "parents": [folder]});
                let mut body = format!(
                    "--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{meta}\r\n--{boundary}\r\nContent-Type: {}\r\n\r\n",
                    mime_of(name)
                )
                .into_bytes();
                body.extend_from_slice(bytes);
                body.extend_from_slice(format!("\r\n--{boundary}--").as_bytes());
                let url = format!("{UPLOAD_API}/files{}", query(&[("uploadType", "multipart"), ("fields", "id,version")]));
                let content_type = format!("multipart/related; boundary={boundary}");
                self.call_json(&|t| Request::new(Method::Post, url.clone()).bearer(t).bytes(&content_type, body.clone()))?
            }
        };
        Ok(WriteOutcome::Written(Revision(written["version"].as_str().unwrap_or_default().to_owned())))
    }

    fn upload(
        &self,
        folder_id: &str,
        name: &str,
        size: u64,
        source: &mut dyn ReadSeek,
        props: &FileProps,
        session: Option<&UploadSession>,
        on_session: &mut dyn FnMut(UploadSession),
        progress: Progress,
    ) -> CloudResult<RemoteFile> {
        // resume where Google says it stands, or start a new session (an old one may have expired: a week)
        let resumed = match session {
            Some(s) => match self.upload_status(&s.url, size) {
                Ok(Ok(offset)) => Some((s.url.clone(), offset)),
                Ok(Err(done)) => return Ok(done),
                Err(CloudError::NotFound) => None,
                Err(e) => return Err(e),
            },
            None => None,
        };
        let (url, mut offset) = match resumed {
            Some(r) => r,
            None => (self.start_upload(folder_id, name, size, props)?, 0),
        };
        on_session(UploadSession { url: url.clone(), offset });
        let mut auth_retry = false;
        loop {
            source.seek(SeekFrom::Start(offset)).map_err(|e| CloudError::Permanent(e.to_string()))?;
            let len = self.chunk.min(size - offset);
            let range = if size == 0 { "bytes */0".to_owned() } else { format!("bytes {offset}-{}/{size}", offset + len - 1) };
            let token = self.token(auth_retry)?;
            let mut body = Vec::new();
            let request = Request::new(Method::Put, url.clone())
                .bearer(&token)
                .header("Content-Range", range)
                .no_redirects()
                .stream(source, len);
            let response = self.http.send(request, &mut body).map_err(transport)?;
            match response.status {
                308 => {
                    offset = confirmed(&response);
                    auth_retry = false;
                    on_session(UploadSession { url: url.clone(), offset });
                    if !progress(offset) {
                        return Err(CloudError::Cancelled);
                    }
                }
                200 | 201 => {
                    progress(size);
                    return Ok(remote_file(&parse(&body)?));
                }
                401 if !auth_retry => auth_retry = true,
                _ => return Err(status_error(&response)),
            }
        }
    }

    fn download(&self, file_id: &str, sink: &mut dyn Write, progress: Progress) -> CloudResult<()> {
        let url = format!("{API}/files/{file_id}?alt=media");
        let mut counting = Counting { inner: sink, done: 0, progress };
        let response = self.call(&|t| Request::new(Method::Get, url.clone()).bearer(t), &mut counting)?;
        if !response.ok() {
            return Err(status_error(&response));
        }
        Ok(())
    }

    fn delete(&self, file_id: &str) -> CloudResult<()> {
        let url = format!("{API}/files/{file_id}");
        self.call_json(&|t| Request::new(Method::Delete, url.clone()).bearer(t)).map(|_| ())
    }

    fn list_tagged(&self, _root_id: &str) -> CloudResult<Vec<RemoteFile>> {
        // drive.file: every file Drive lists for Lag is Lag's own
        let mut files = Vec::new();
        let mut page: Option<String> = None;
        loop {
            let mut params = vec![
                ("q", "trashed = false".to_owned()),
                ("fields", format!("nextPageToken,files({FILE_FIELDS})")),
                ("pageSize", "1000".to_owned()),
                ("spaces", "drive".to_owned()),
            ];
            if let Some(p) = &page {
                params.push(("pageToken", p.clone()));
            }
            let pairs: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
            let url = format!("{API}/files{}", query(&pairs));
            let answer = self.call_json(&|t| Request::new(Method::Get, url.clone()).bearer(t))?;
            for f in answer["files"].as_array().into_iter().flatten() {
                let file = remote_file(f);
                if file.props.contains_key(props::CLIP_ID) {
                    files.push(file);
                }
            }
            match answer["nextPageToken"].as_str() {
                Some(next) => page = Some(next.to_owned()),
                None => return Ok(files),
            }
        }
    }

    fn space(&self) -> CloudResult<ProviderSpace> {
        let url = format!("{API}/about?fields=storageQuota");
        let answer = self.call_json(&|t| Request::new(Method::Get, url.clone()).bearer(t))?;
        let number = |k: &str| answer["storageQuota"][k].as_str().and_then(|s| s.parse::<u64>().ok());
        let usage = number("usage").unwrap_or(0);
        Ok(match number("limit") {
            Some(limit) => ProviderSpace { free: limit.saturating_sub(usage), total: Some(limit) },
            None => ProviderSpace { free: u64::MAX, total: None },
        })
    }

    fn stream(&self, file_id: &str) -> CloudResult<StreamSource> {
        let token = self.token(false)?;
        Ok(StreamSource {
            url: format!("{API}/files/{file_id}?alt=media"),
            headers: vec![("Authorization".into(), format!("Bearer {token}"))],
        })
    }

    fn web_link(&self, file_id: &str) -> Option<String> {
        Some(format!("https://drive.google.com/file/d/{file_id}/view"))
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::fake::FakeHttp;
    use chrono::Duration;
    use std::io::Cursor;

    type Reply = (u16, Vec<(&'static str, String)>, Vec<u8>);

    fn drive(http: &Arc<FakeHttp>, expires_in_secs: i64) -> GoogleDrive {
        let tokens = Tokens {
            access_token: "at".into(),
            refresh_token: "rt".into(),
            expires_at: Utc::now() + Duration::seconds(expires_in_secs),
            sub: "42".into(),
            email: "me@example.com".into(),
        };
        let client = ClientConfig { id: "cid".into(), secret: "s".into() };
        GoogleDrive::new(http.clone(), client, tokens, |_| {})
    }

    fn ok_json(v: Value) -> Reply {
        (200, vec![], v.to_string().into_bytes())
    }

    fn path(p: &[&str]) -> Vec<String> {
        p.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn folders_are_found_or_created_and_cached() {
        let http = Arc::new(FakeHttp::default());
        let d = drive(&http, 3600);
        http.expect(|r| {
            assert!(r.url.contains("name%20%3D%20%27Lag%27") && r.url.contains("%27root%27%20in%20parents"), "{}", r.url);
            ok_json(json!({"files": [{"id": "lag", "version": "1"}]}))
        });
        http.expect(|_| ok_json(json!({"files": []})));
        http.expect(|r| {
            assert_eq!(r.method, Method::Post);
            assert_eq!(r.json()["name"], "Tom Clancy's");
            assert_eq!(r.json()["parents"][0], "lag");
            assert_eq!(r.json()["mimeType"], FOLDER_MIME);
            ok_json(json!({"id": "game"}))
        });
        assert_eq!(d.ensure_folder(&path(&["Lag", "Tom Clancy's"])).unwrap(), "game");
        // the second time it comes from the cache: no request
        assert_eq!(d.ensure_folder(&path(&["Lag", "Tom Clancy's"])).unwrap(), "game");
        assert_eq!(http.remaining(), 0);
        assert!(http.log.lock().unwrap()[1].url.contains("Tom%20Clancy%5C%27s"), "quote escaped in the query");
    }

    #[test]
    fn a_resumable_upload_goes_in_chunks_and_resumes_where_google_stands() {
        let http = Arc::new(FakeHttp::default());
        let d = drive(&http, 3600).with_chunk(4);
        let data = b"0123456789".to_vec();
        http.expect(|r| {
            assert!(r.url.contains("uploadType=resumable"));
            assert_eq!(r.header("X-Upload-Content-Length"), Some("10"));
            assert_eq!(r.json()["appProperties"]["lagClipId"], "c1");
            (200, vec![("Location", "https://upload/session1".into())], vec![])
        });
        http.expect(|r| {
            assert_eq!((r.header("Content-Range"), r.body.as_slice()), (Some("bytes 0-3/10"), &b"0123"[..]));
            assert!(!r.follow_redirects, "308 is not a redirect");
            (308, vec![("Range", "bytes=0-3".into())], vec![])
        });
        // the connection drops: Google answers 503
        http.expect(|_| (503, vec![], br#"{"error":{"message":"backend"}}"#.to_vec()));
        let mut props = FileProps::new();
        props.insert(props::CLIP_ID.into(), "c1".into());
        let mut sessions = Vec::new();
        let err = d
            .upload("folder", "a.mp4", 10, &mut Cursor::new(&data), &props, None, &mut |s| sessions.push(s), &mut |_| true)
            .unwrap_err();
        assert!(err.is_retryable(), "{err:?}");
        let stored = sessions.last().unwrap().clone();
        assert_eq!(stored, UploadSession { url: "https://upload/session1".into(), offset: 4 });

        // the retry: Google actually has 6 bytes (part of the failed chunk arrived)
        http.expect(|r| {
            assert_eq!(r.header("Content-Range"), Some("bytes */10"));
            (308, vec![("Range", "bytes=0-5".into())], vec![])
        });
        http.expect(|r| {
            assert_eq!((r.header("Content-Range"), r.body.as_slice()), (Some("bytes 6-9/10"), &b"6789"[..]));
            ok_json(json!({"id": "f1", "name": "a.mp4", "size": "10", "appProperties": {"lagClipId": "c1"}}))
        });
        let file = d
            .upload("folder", "a.mp4", 10, &mut Cursor::new(&data), &props, Some(&stored), &mut |_| {}, &mut |_| true)
            .unwrap();
        assert_eq!((file.id.as_str(), file.size, file.props[props::CLIP_ID].as_str()), ("f1", 10, "c1"));
    }

    #[test]
    fn an_expired_session_starts_over() {
        let http = Arc::new(FakeHttp::default());
        let d = drive(&http, 3600).with_chunk(8);
        http.expect(|_| (404, vec![], vec![]));
        http.expect(|_| (200, vec![("Location", "https://upload/new".into())], vec![]));
        http.expect(|r| {
            assert_eq!(r.header("Content-Range"), Some("bytes 0-2/3"));
            ok_json(json!({"id": "f", "size": "3"}))
        });
        let old = UploadSession { url: "https://upload/old".into(), offset: 2 };
        let file = d
            .upload("d", "x.mp4", 3, &mut Cursor::new(b"abc"), &FileProps::new(), Some(&old), &mut |_| {}, &mut |_| true)
            .unwrap();
        assert_eq!(file.id, "f");
    }

    #[test]
    fn google_errors_map_to_what_lag_does_next() {
        let error = |status: u16, reason: &str| {
            let body = json!({"error": {"errors": [{"reason": reason}], "message": "m"}}).to_string().into_bytes();
            status_error(&Response { status, headers: vec![], error_body: body })
        };
        assert_eq!(error(403, "storageQuotaExceeded"), CloudError::Full);
        assert!(error(403, "userRateLimitExceeded").is_retryable());
        assert!(error(429, "").is_retryable());
        assert!(error(502, "").is_retryable());
        assert_eq!(error(404, "notFound"), CloudError::NotFound);
        assert_eq!(error(401, ""), CloudError::SignIn);
        assert!(matches!(error(403, "insufficientFilePermissions"), CloudError::Permanent(_)));
    }

    #[test]
    fn tokens_refresh_before_expiry_and_after_a_401() {
        let http = Arc::new(FakeHttp::default());
        let saved = Arc::new(Mutex::new(Vec::<String>::new()));
        let tokens = Tokens {
            access_token: "old".into(),
            refresh_token: "rt".into(),
            expires_at: Utc::now() + Duration::seconds(10),
            sub: "42".into(),
            email: String::new(),
        };
        let s2 = saved.clone();
        let d = GoogleDrive::new(http.clone(), ClientConfig { id: "c".into(), secret: "s".into() }, tokens, move |t| {
            s2.lock().unwrap().push(t.access_token.clone())
        });
        // expires in 10 s: refreshed first
        http.expect(|r| {
            assert!(r.url.contains("oauth2.googleapis.com/token"));
            ok_json(json!({"access_token": "fresh", "expires_in": 3600}))
        });
        // Drive still says 401 (revoked server-side): one more refresh, then the call succeeds
        http.expect(|r| {
            assert_eq!(r.header("Authorization"), Some("Bearer fresh"));
            (401, vec![], vec![])
        });
        http.expect(|_| ok_json(json!({"access_token": "fresher", "expires_in": 3600})));
        http.expect(|r| {
            assert_eq!(r.header("Authorization"), Some("Bearer fresher"));
            ok_json(json!({"storageQuota": {"limit": "16106127360", "usage": "6106127360"}}))
        });
        let space = d.space().unwrap();
        assert_eq!((space.free, space.total), (10_000_000_000, Some(16_106_127_360)));
        assert_eq!(*saved.lock().unwrap(), ["fresh", "fresher"], "every new token is stored");
        // a revoked refresh token = sign in again
        http.expect(|_| (401, vec![], vec![]));
        http.expect(|_| (400, vec![], br#"{"error":"invalid_grant"}"#.to_vec()));
        assert_eq!(d.space().unwrap_err(), CloudError::SignIn);
    }

    #[test]
    fn the_index_is_written_only_if_nobody_changed_it() {
        let http = Arc::new(FakeHttp::default());
        let d = drive(&http, 3600);
        let index = path(&["Lag", ".lag", "library.json"]);
        // folders Lag/.lag exist
        http.expect(|_| ok_json(json!({"files": [{"id": "lag"}]})));
        http.expect(|_| ok_json(json!({"files": [{"id": "sys"}]})));
        // the index exists at version 7, but we expected 5: conflict, nothing written
        http.expect(|_| ok_json(json!({"files": [{"id": "idx", "version": "7"}]})));
        assert_eq!(d.write_small(&index, b"{}", Some(&Revision("5".into()))).unwrap(), WriteOutcome::Conflict);
        // expected 7: PATCH the content
        http.expect(|_| ok_json(json!({"files": [{"id": "idx", "version": "7"}]})));
        http.expect(|r| {
            assert_eq!(r.method, Method::Patch);
            assert!(r.url.contains("/upload/drive/v3/files/idx") && r.url.contains("uploadType=media"));
            assert_eq!(r.body, b"{\"a\":1}");
            ok_json(json!({"id": "idx", "version": "8"}))
        });
        assert_eq!(
            d.write_small(&index, b"{\"a\":1}", Some(&Revision("7".into()))).unwrap(),
            WriteOutcome::Written(Revision("8".into()))
        );
        // reading it back: content + revision
        http.expect(|_| ok_json(json!({"files": [{"id": "idx", "version": "8"}]})));
        http.expect(|r| {
            assert!(r.url.ends_with("files/idx?alt=media"));
            (200, vec![], b"{\"a\":1}".to_vec())
        });
        assert_eq!(d.read_small(&index).unwrap(), Some((b"{\"a\":1}".to_vec(), Revision("8".into()))));
    }

    #[test]
    fn a_new_index_is_created_with_a_multipart_upload() {
        let http = Arc::new(FakeHttp::default());
        let d = drive(&http, 3600);
        http.expect(|_| ok_json(json!({"files": [{"id": "lag"}]})));
        http.expect(|_| ok_json(json!({"files": [{"id": "sys"}]})));
        http.expect(|_| ok_json(json!({"files": []})));
        http.expect(|r| {
            assert!(r.url.contains("uploadType=multipart"));
            let body = String::from_utf8_lossy(&r.body).into_owned();
            assert!(body.contains("\"parents\":[\"sys\"]") && body.contains("library.json") && body.contains("{}"), "{body}");
            ok_json(json!({"id": "idx", "version": "1"}))
        });
        let index = path(&["Lag", ".lag", "library.json"]);
        assert_eq!(d.write_small(&index, b"{}", None).unwrap(), WriteOutcome::Written(Revision("1".into())));
    }

    #[test]
    fn downloads_never_write_an_error_into_the_file() {
        let http = Arc::new(FakeHttp::default());
        let d = drive(&http, 3600);
        http.expect(|_| (200, vec![], b"video-bytes".to_vec()));
        let mut file = Vec::new();
        let mut seen = 0;
        d.download("f1", &mut file, &mut |n| {
            seen = n;
            true
        })
        .unwrap();
        assert_eq!((file.as_slice(), seen), (&b"video-bytes"[..], 11));
        http.expect(|_| (404, vec![], br#"{"error":{"message":"File not found"}}"#.to_vec()));
        let mut file = Vec::new();
        assert_eq!(d.download("gone", &mut file, &mut |_| true).unwrap_err(), CloudError::NotFound);
        assert!(file.is_empty());
        // the stream URL plays with the token as a header
        let s = d.stream("f1").unwrap();
        assert!(s.url.ends_with("files/f1?alt=media") && s.headers[0].1 == "Bearer at");
    }

    #[test]
    fn listing_finds_lags_clips_over_several_pages() {
        let http = Arc::new(FakeHttp::default());
        let d = drive(&http, 3600);
        http.expect(|_| {
            ok_json(json!({"nextPageToken": "p2", "files": [
                {"id": "a", "name": "a.mp4", "size": "5", "appProperties": {"lagClipId": "x", "lagKind": "clip"}},
                {"id": "idx", "name": "library.json", "size": "2"}
            ]}))
        });
        http.expect(|r| {
            assert!(r.url.contains("pageToken=p2"));
            ok_json(json!({"files": [{"id": "t", "name": "x.jpg", "size": "1", "appProperties": {"lagClipId": "x", "lagKind": "thumb"}}]}))
        });
        let files = d.list_tagged("root").unwrap();
        assert_eq!(files.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["a", "t"]);
    }
}
