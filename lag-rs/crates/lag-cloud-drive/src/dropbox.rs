//! Dropbox (API v2) as a cloud, with "App folder" access: everything Lag does happens inside `Apps/<app name>`
//! in the user's Dropbox, nothing else is visible to it. Paths are Dropbox paths (`/Lag/Counter-Strike 2`):
//! a file's id is its path. Uploads go in 8 MB upload-session chunks and resume where Dropbox says it stands;
//! the index is written with `update` mode + `strict_conflict`, so two PCs never overwrite each other. Lag's
//! file properties are notes in `.lag/props` (sidecar.rs).
use crate::http::{Method, Request, Response};
use crate::session::{ascii_json, Counting, OAuthSession};
use crate::sidecar;
use lag_cloud::provider::*;
use lag_cloud::queue::UploadSession;
use lag_cloud::quota::ProviderSpace;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::{SeekFrom, Write};
use std::sync::Mutex;

const RPC: &str = "https://api.dropboxapi.com/2";
const CONTENT: &str = "https://content.dropboxapi.com/2";
pub const CHUNK: u64 = 8 << 20;

pub struct Dropbox {
    session: OAuthSession,
    made: Mutex<HashSet<String>>,
    chunk: u64,
}

fn path_of(segments: &[String]) -> String {
    segments.iter().map(|s| format!("/{s}")).collect()
}

/// Dropbox's error answer as a `CloudError` (409 = an API error described by `error_summary`).
fn status_error(r: &Response) -> CloudError {
    let v: Value = serde_json::from_slice(&r.error_body).unwrap_or(Value::Null);
    let summary = v["error_summary"].as_str().unwrap_or("");
    match r.status {
        401 => CloudError::SignIn,
        409 if summary.contains("insufficient_space") => CloudError::Full,
        409 if summary.contains("not_found") => CloudError::NotFound,
        507 => CloudError::Full,
        429 | 500..=599 => CloudError::Transient(format!("HTTP {} {summary}", r.status)),
        s => CloudError::Permanent(format!("HTTP {s} {summary}")),
    }
}

impl Dropbox {
    pub fn new(session: OAuthSession) -> Self {
        Self { session, made: Mutex::default(), chunk: CHUNK }
    }

    #[cfg(test)]
    fn with_chunk(mut self, chunk: u64) -> Self {
        self.chunk = chunk;
        self
    }

    fn rpc(&self, endpoint: &str, arg: Value) -> CloudResult<(Response, Vec<u8>)> {
        let url = format!("{RPC}{endpoint}");
        let body = arg.to_string().into_bytes();
        let mut out = Vec::new();
        let r = self.session.call(&|t| Request::new(Method::Post, url.clone()).bearer(t).bytes("application/json", body.clone()), &mut out)?;
        Ok((r, out))
    }

    /// A content endpoint: `arg` in the Dropbox-API-Arg header, `bytes` as the body.
    fn content(&self, endpoint: &str, arg: &Value, bytes: Vec<u8>, sink: &mut dyn Write) -> CloudResult<Response> {
        let url = format!("{CONTENT}{endpoint}");
        let header = ascii_json(arg);
        self.session.call(
            &|t| {
                Request::new(Method::Post, url.clone())
                    .bearer(t)
                    .header("Dropbox-API-Arg", header.clone())
                    .bytes("application/octet-stream", bytes.clone())
            },
            sink,
        )
    }

    fn parse(body: &[u8]) -> CloudResult<Value> {
        serde_json::from_slice(body).map_err(|e| CloudError::Permanent(format!("bad answer from Dropbox: {e}")))
    }

    fn write_note(&self, file_path: &str, name: &str, size: u64, props: &FileProps) -> CloudResult<()> {
        if !sidecar::tagged(props) {
            return Ok(());
        }
        let rel = file_path.trim_start_matches('/').to_owned();
        let root = rel.split('/').next().unwrap_or("");
        let note_path = format!("/{root}/{}/{}", sidecar::DIR.join("/"), sidecar::name(&rel));
        let note = sidecar::Note { path: rel.clone(), name: name.to_owned(), size, props: props.clone() };
        let arg = json!({"path": note_path, "mode": "overwrite", "autorename": false, "mute": true});
        let r = self.content("/files/upload", &arg, serde_json::to_vec(&note).unwrap_or_default(), &mut std::io::sink())?;
        if r.ok() {
            Ok(())
        } else {
            Err(status_error(&r))
        }
    }
}

impl CloudProvider for Dropbox {
    fn ensure_folder(&self, path: &[String]) -> CloudResult<String> {
        let full = path_of(path);
        if path.is_empty() || self.made.lock().unwrap().contains(&full) {
            return Ok(full);
        }
        // Dropbox makes the parents too
        let (r, _) = self.rpc("/files/create_folder_v2", json!({"path": full, "autorename": false}))?;
        let exists = r.status == 409 && String::from_utf8_lossy(&r.error_body).contains("path/conflict");
        if !(r.ok() || exists) {
            return Err(status_error(&r));
        }
        self.made.lock().unwrap().insert(full.clone());
        Ok(full)
    }

    fn read_small(&self, path: &[String]) -> CloudResult<Option<(Vec<u8>, Revision)>> {
        let mut body = Vec::new();
        let r = self.content("/files/download", &json!({"path": path_of(path)}), Vec::new(), &mut body)?;
        if r.status == 409 && String::from_utf8_lossy(&r.error_body).contains("not_found") {
            return Ok(None);
        }
        if !r.ok() {
            return Err(status_error(&r));
        }
        let meta: Value = r.header("dropbox-api-result").and_then(|h| serde_json::from_str(h).ok()).unwrap_or(Value::Null);
        Ok(Some((body, Revision(meta["rev"].as_str().unwrap_or_default().to_owned()))))
    }

    fn write_small(&self, path: &[String], bytes: &[u8], expected: Option<&Revision>) -> CloudResult<WriteOutcome> {
        let mode = match expected {
            Some(rev) => json!({".tag": "update", "update": rev.0}),
            None => json!({".tag": "add"}),
        };
        let arg = json!({"path": path_of(path), "mode": mode, "autorename": false, "mute": true, "strict_conflict": true});
        let mut body = Vec::new();
        let r = self.content("/files/upload", &arg, bytes.to_vec(), &mut body)?;
        if r.status == 409 && String::from_utf8_lossy(&r.error_body).contains("conflict") {
            return Ok(WriteOutcome::Conflict);
        }
        if !r.ok() {
            return Err(status_error(&r));
        }
        Ok(WriteOutcome::Written(Revision(Self::parse(&body)?["rev"].as_str().unwrap_or_default().to_owned())))
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
        let path = format!("{folder_id}/{name}");
        let (mut id, mut offset) = match session {
            Some(s) if !s.url.is_empty() => (Some(s.url.clone()), s.offset),
            _ => (None, 0),
        };
        let mut buf = vec![0u8; self.chunk as usize];
        loop {
            let n = self.chunk.min(size - offset) as usize;
            source.seek(SeekFrom::Start(offset)).map_err(|e| CloudError::Permanent(e.to_string()))?;
            source.read_exact(&mut buf[..n]).map_err(|e| CloudError::Permanent(e.to_string()))?;
            let last = offset + n as u64 == size;
            let mut answer = Vec::new();
            let r = match &id {
                None => self.content("/files/upload_session/start", &json!({"close": false}), buf[..n].to_vec(), &mut answer)?,
                Some(sid) if last => {
                    let arg = json!({
                        "cursor": {"session_id": sid, "offset": offset},
                        "commit": {"path": path, "mode": "overwrite", "autorename": false, "mute": true}
                    });
                    self.content("/files/upload_session/finish", &arg, buf[..n].to_vec(), &mut answer)?
                }
                Some(sid) => {
                    let arg = json!({"cursor": {"session_id": sid, "offset": offset}, "close": false});
                    self.content("/files/upload_session/append_v2", &arg, buf[..n].to_vec(), &mut answer)?
                }
            };
            if r.status == 409 {
                let error: Value = serde_json::from_slice(&r.error_body).unwrap_or(Value::Null);
                let summary = error["error_summary"].as_str().unwrap_or("");
                // Dropbox has a different amount than we thought: go on from its number
                if summary.contains("incorrect_offset") {
                    if let Some(correct) = error["error"]["correct_offset"].as_u64().or(error["error"]["lookup_failed"]["correct_offset"].as_u64()) {
                        offset = correct;
                        continue;
                    }
                }
                // the session expired (a week) or is unknown: start over
                if summary.contains("not_found") && id.is_some() {
                    id = None;
                    offset = 0;
                    continue;
                }
            }
            if !r.ok() {
                return Err(status_error(&r));
            }
            if id.is_none() {
                id = Some(Self::parse(&answer)?["session_id"].as_str().unwrap_or_default().to_owned());
                if last {
                    // a one-chunk file: the session still needs its commit
                    let sid = id.clone().unwrap_or_default();
                    let arg = json!({
                        "cursor": {"session_id": sid, "offset": size},
                        "commit": {"path": path, "mode": "overwrite", "autorename": false, "mute": true}
                    });
                    answer.clear();
                    let r = self.content("/files/upload_session/finish", &arg, Vec::new(), &mut answer)?;
                    if !r.ok() {
                        return Err(status_error(&r));
                    }
                }
            }
            offset += n as u64;
            on_session(UploadSession { url: id.clone().unwrap_or_default(), offset });
            if !progress(offset) {
                return Err(CloudError::Cancelled);
            }
            if last {
                let meta = Self::parse(&answer)?;
                let id = meta["path_display"].as_str().map(str::to_owned).unwrap_or(path.clone());
                self.write_note(&id, name, size, props)?;
                return Ok(RemoteFile {
                    id,
                    name: name.to_owned(),
                    size,
                    hash: meta["content_hash"].as_str().map(str::to_owned),
                    props: props.clone(),
                });
            }
        }
    }

    fn download(&self, file_id: &str, sink: &mut dyn Write, progress: Progress) -> CloudResult<()> {
        let mut counting = Counting { inner: sink, done: 0, progress };
        let r = self.content("/files/download", &json!({"path": file_id}), Vec::new(), &mut counting)?;
        if r.ok() {
            Ok(())
        } else {
            Err(status_error(&r))
        }
    }

    fn delete(&self, file_id: &str) -> CloudResult<()> {
        let (r, _) = self.rpc("/files/delete_v2", json!({"path": file_id}))?;
        if !r.ok() {
            return Err(status_error(&r));
        }
        let rel = file_id.trim_start_matches('/');
        let root = rel.split('/').next().unwrap_or("");
        let note = format!("/{root}/{}/{}", sidecar::DIR.join("/"), sidecar::name(rel));
        let _ = self.rpc("/files/delete_v2", json!({"path": note}));
        Ok(())
    }

    fn list_tagged(&self, root_id: &str) -> CloudResult<Vec<RemoteFile>> {
        let mut notes = Vec::new();
        let (mut r, mut body) = self.rpc("/files/list_folder", json!({"path": format!("{root_id}/{}", sidecar::DIR.join("/"))}))?;
        loop {
            if r.status == 409 {
                return Ok(Vec::new());
            }
            if !r.ok() {
                return Err(status_error(&r));
            }
            let page = Self::parse(&body)?;
            for e in page["entries"].as_array().into_iter().flatten().filter(|e| e[".tag"] == "file") {
                if let Some(p) = e["path_display"].as_str() {
                    notes.push(p.to_owned());
                }
            }
            if page["has_more"].as_bool() != Some(true) {
                break;
            }
            (r, body) = self.rpc("/files/list_folder/continue", json!({"cursor": page["cursor"]}))?;
        }
        let mut files = Vec::new();
        for note_path in notes {
            let mut bytes = Vec::new();
            let r = self.content("/files/download", &json!({"path": note_path}), Vec::new(), &mut bytes)?;
            let Ok(note) = serde_json::from_slice::<sidecar::Note>(&bytes) else { continue };
            if !r.ok() {
                continue;
            }
            files.push(RemoteFile {
                id: format!("/{}", note.path),
                name: if note.name.is_empty() { note.path.rsplit('/').next().unwrap_or_default().to_owned() } else { note.name },
                size: note.size,
                hash: None,
                props: note.props,
            });
        }
        Ok(files)
    }

    fn space(&self) -> CloudResult<ProviderSpace> {
        let (r, body) = self.rpc("/users/get_space_usage", Value::Null)?;
        if !r.ok() {
            return Err(status_error(&r));
        }
        let v = Self::parse(&body)?;
        let used = v["used"].as_u64().unwrap_or(0);
        Ok(match v["allocation"]["allocated"].as_u64() {
            Some(total) => ProviderSpace { free: total.saturating_sub(used), total: Some(total) },
            None => ProviderSpace { free: u64::MAX, total: None },
        })
    }

    fn stream(&self, file_id: &str) -> CloudResult<StreamSource> {
        // a temporary link (four hours): the player needs no header
        let (r, body) = self.rpc("/files/get_temporary_link", json!({"path": file_id}))?;
        if !r.ok() {
            return Err(status_error(&r));
        }
        let link = Self::parse(&body)?["link"].as_str().unwrap_or_default().to_owned();
        Ok(StreamSource { url: link, headers: Vec::new() })
    }

    fn web_link(&self, _file_id: &str) -> Option<String> {
        Some("https://www.dropbox.com/home/Apps".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::fake::{FakeHttp, Seen};
    use crate::oauth::{ClientConfig, Tokens, DROPBOX};
    use chrono::{Duration, Utc};
    use std::io::Cursor;
    use std::sync::Arc;

    fn dropbox(http: &Arc<FakeHttp>) -> Dropbox {
        let tokens = Tokens {
            access_token: "at".into(),
            refresh_token: "rt".into(),
            expires_at: Utc::now() + Duration::hours(1),
            sub: "dbid:1".into(),
            email: String::new(),
        };
        let client = ClientConfig { id: "key".into(), secret: String::new() };
        Dropbox::new(OAuthSession::new(http.clone(), &DROPBOX, client, tokens, |_| {}))
    }

    fn arg(r: &Seen) -> Value {
        serde_json::from_str(r.header("Dropbox-API-Arg").unwrap()).unwrap()
    }

    fn ok(v: Value) -> (u16, Vec<(&'static str, String)>, Vec<u8>) {
        (200, vec![], v.to_string().into_bytes())
    }

    #[test]
    fn folders_and_the_index_with_revisions() {
        let http = Arc::new(FakeHttp::default());
        let d = dropbox(&http);
        http.expect(|r| {
            assert!(r.url.ends_with("/files/create_folder_v2"));
            assert_eq!(r.json()["path"], "/Lag/Сталкер 2");
            (409, vec![], br#"{"error_summary":"path/conflict/folder/"}"#.to_vec())
        });
        let path = vec!["Lag".to_string(), "Сталкер 2".to_string()];
        assert_eq!(d.ensure_folder(&path).unwrap(), "/Lag/Сталкер 2", "an existing folder is fine");
        d.ensure_folder(&path).unwrap();

        let index: Vec<String> = ["Lag", ".lag", "library.json"].iter().map(|s| s.to_string()).collect();
        http.expect(|r| {
            assert!(r.header("Dropbox-API-Arg").unwrap().is_ascii());
            assert_eq!(arg(r)["mode"][".tag"], "add");
            ok(json!({"rev": "r1"}))
        });
        assert_eq!(d.write_small(&index, b"{}", None).unwrap(), WriteOutcome::Written(Revision("r1".into())));
        http.expect(|r| {
            assert_eq!((arg(r)["mode"][".tag"].as_str(), arg(r)["mode"]["update"].as_str()), (Some("update"), Some("r1")));
            (409, vec![], br#"{"error_summary":"path/conflict/file/"}"#.to_vec())
        });
        assert_eq!(d.write_small(&index, b"{}", Some(&Revision("r1".into()))).unwrap(), WriteOutcome::Conflict);
        http.expect(|_| (200, vec![("Dropbox-API-Result", r#"{"rev":"r2"}"#.into())], b"{\"a\":1}".to_vec()));
        assert_eq!(d.read_small(&index).unwrap(), Some((b"{\"a\":1}".to_vec(), Revision("r2".into()))));
        http.expect(|_| (409, vec![], br#"{"error_summary":"path/not_found/"}"#.to_vec()));
        assert_eq!(d.read_small(&index).unwrap(), None);
    }

    #[test]
    fn uploads_go_in_sessions_and_resume_at_dropboxs_offset() {
        let http = Arc::new(FakeHttp::default());
        let d = dropbox(&http).with_chunk(4);
        let data = b"0123456789".to_vec();
        let mut props = FileProps::new();
        props.insert(props::CLIP_ID.into(), "c1".into());
        http.expect(|r| {
            assert!(r.url.ends_with("/upload_session/start") && r.body == b"0123");
            ok(json!({"session_id": "S"}))
        });
        // the connection drops on the second chunk
        http.expect(|_| (503, vec![], vec![]));
        let mut sessions = Vec::new();
        let err = d
            .upload("/Lag/Desktop", "a.mp4", 10, &mut Cursor::new(&data), &props, None, &mut |s| sessions.push(s), &mut |_| true)
            .unwrap_err();
        assert!(err.is_retryable());
        let stored = sessions.last().unwrap().clone();
        assert_eq!(stored, UploadSession { url: "S".into(), offset: 4 });

        // the retry: Dropbox actually got 6 bytes, so 6..10 is the last chunk: finish, not append
        let http2 = Arc::new(FakeHttp::default());
        let d2 = dropbox(&http2).with_chunk(4);
        http2.expect(|_| (409, vec![], br#"{"error_summary":"incorrect_offset/","error":{"correct_offset":6}}"#.to_vec()));
        http2.expect(|r| {
            assert!(r.url.ends_with("/upload_session/finish"), "{}", r.url);
            assert_eq!((arg(r)["cursor"]["offset"].as_u64(), r.body.as_slice()), (Some(6), &b"6789"[..]));
            assert_eq!(arg(r)["commit"]["path"], "/Lag/Desktop/a.mp4");
            ok(json!({"path_display": "/Lag/Desktop/a.mp4", "content_hash": "h"}))
        });
        http2.expect(|r| {
            assert!(r.url.ends_with("/files/upload"));
            assert!(arg(r)["path"].as_str().unwrap().starts_with("/Lag/.lag/props/"));
            ok(json!({"rev": "n"}))
        });
        let file = d2
            .upload("/Lag/Desktop", "a.mp4", 10, &mut Cursor::new(&data), &props, Some(&stored), &mut |_| {}, &mut |_| true)
            .unwrap();
        assert_eq!((file.id.as_str(), file.hash.as_deref()), ("/Lag/Desktop/a.mp4", Some("h")));
        assert_eq!(http2.remaining(), 0);
    }

    #[test]
    fn a_small_file_is_one_chunk_plus_commit() {
        let http = Arc::new(FakeHttp::default());
        let d = dropbox(&http);
        http.expect(|_| ok(json!({"session_id": "S"})));
        http.expect(|r| {
            assert!(r.url.ends_with("/upload_session/finish") && r.body.is_empty());
            assert_eq!(arg(r)["cursor"]["offset"], 3);
            ok(json!({"path_display": "/Lag/x.jpg"}))
        });
        let f = d.upload("/Lag", "x.jpg", 3, &mut Cursor::new(b"abc"), &FileProps::new(), None, &mut |_| {}, &mut |_| true).unwrap();
        assert_eq!(f.id, "/Lag/x.jpg");
    }

    #[test]
    fn space_stream_and_errors() {
        let http = Arc::new(FakeHttp::default());
        let d = dropbox(&http);
        http.expect(|_| ok(json!({"used": 1000, "allocation": {".tag": "individual", "allocated": 5000}})));
        assert_eq!(d.space().unwrap(), ProviderSpace { free: 4000, total: Some(5000) });
        http.expect(|_| ok(json!({"link": "https://dl.dropboxusercontent.com/x"})));
        let s = d.stream("/Lag/a.mp4").unwrap();
        assert!(s.url.starts_with("https://dl.") && s.headers.is_empty());
        http.expect(|_| (409, vec![], br#"{"error_summary":"path/insufficient_space/"}"#.to_vec()));
        let err = d.upload("/Lag", "b.mp4", 1, &mut Cursor::new(b"x"), &FileProps::new(), None, &mut |_| {}, &mut |_| true).unwrap_err();
        assert_eq!(err, CloudError::Full);
    }
}
