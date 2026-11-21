//! OneDrive (Microsoft Graph) as a cloud, in the app's own folder (`Apps/Lag`, scope Files.ReadWrite.AppFolder):
//! nothing else in the user's OneDrive is visible to Lag. Folders are found by path and cached with their ids;
//! a file's id is its item id. Uploads go through an upload session in 10 MiB chunks (multiples of 320 KiB, as
//! Graph asks) and resume from the range Graph still expects; the index is written with `If-Match` on its eTag
//! (`conflictBehavior=fail` when new). Files are fetched through their pre-signed download URL (no token sent
//! to the storage host). Lag's file properties are notes in `.lag-props` at the app folder's top (sidecar.rs),
//! named by item id.
use crate::http::{Method, Request, Response};
use crate::session::{transport, Counting, OAuthSession};
use crate::sidecar;
use lag_cloud::provider::*;
use lag_cloud::queue::UploadSession;
use lag_cloud::quota::ProviderSpace;
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{SeekFrom, Write};
use std::sync::Mutex;

const APPROOT: &str = "https://graph.microsoft.com/v1.0/me/drive/special/approot";
const ITEMS: &str = "https://graph.microsoft.com/v1.0/me/drive/items";
/// Folder of the notes, at the top of the app folder.
const NOTES: &str = ".lag-props";
/// 32 x 320 KiB.
pub const CHUNK: u64 = 32 * 320 * 1024;
const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');

pub struct OneDrive {
    session: OAuthSession,
    folders: Mutex<HashMap<Vec<String>, String>>,
    chunk: u64,
}

fn by_path(path: &[String]) -> String {
    let encoded: Vec<String> = path.iter().map(|s| utf8_percent_encode(s, SEGMENT).to_string()).collect();
    format!("{APPROOT}:/{}:", encoded.join("/"))
}

fn status_error(r: &Response) -> CloudError {
    let v: Value = serde_json::from_slice(&r.error_body).unwrap_or(Value::Null);
    let code = v["error"]["code"].as_str().unwrap_or("");
    match r.status {
        401 => CloudError::SignIn,
        404 | 410 => CloudError::NotFound,
        507 => CloudError::Full,
        _ if code == "quotaLimitReached" || code == "insufficientStorage" => CloudError::Full,
        408 | 429 | 500..=599 => CloudError::Transient(format!("HTTP {} {code}", r.status)),
        s => CloudError::Permanent(format!("HTTP {s} {code}")),
    }
}

fn parse(body: &[u8]) -> CloudResult<Value> {
    serde_json::from_slice(body).map_err(|e| CloudError::Permanent(format!("bad answer from OneDrive: {e}")))
}

/// The first byte Graph still expects: `["26-"]` -> 26.
fn next_expected(v: &Value) -> Option<u64> {
    v["nextExpectedRanges"].as_array()?.first()?.as_str()?.split('-').next()?.parse().ok()
}

impl OneDrive {
    pub fn new(session: OAuthSession) -> Self {
        Self { session, folders: Mutex::default(), chunk: CHUNK }
    }

    #[cfg(test)]
    fn with_chunk(mut self, chunk: u64) -> Self {
        self.chunk = chunk;
        self
    }

    /// A signed request; None on 404.
    fn get(&self, url: &str) -> CloudResult<Option<Value>> {
        let mut body = Vec::new();
        let r = self.session.call(&|t| Request::new(Method::Get, url.to_owned()).bearer(t), &mut body)?;
        match r.status {
            200..=299 => Ok(Some(parse(&body)?)),
            404 => Ok(None),
            _ => Err(status_error(&r)),
        }
    }

    /// A pre-signed URL (download, upload session): the storage host gets no token.
    fn plain(&self, request: Request, sink: &mut dyn Write) -> CloudResult<Response> {
        self.session.http.send(request, sink).map_err(transport)
    }

    fn download_url(&self, item_url: &str) -> CloudResult<Option<(String, Value)>> {
        let Some(item) = self.get(&format!("{item_url}?select=id,eTag,size,@microsoft.graph.downloadUrl"))? else {
            return Ok(None);
        };
        let url = item["@microsoft.graph.downloadUrl"].as_str().unwrap_or_default().to_owned();
        Ok(Some((url, item)))
    }

    fn note_path(item_id: &str) -> Vec<String> {
        vec![NOTES.to_owned(), sidecar::name(item_id)]
    }

    fn put_small(&self, path: &[String], bytes: &[u8]) -> CloudResult<()> {
        let url = format!("{}/content", by_path(path));
        let r = self.session.call(&|t| Request::new(Method::Put, url.clone()).bearer(t).bytes("application/json", bytes.to_vec()), &mut std::io::sink())?;
        if r.ok() {
            Ok(())
        } else {
            Err(status_error(&r))
        }
    }

    fn new_session(&self, folder_id: &str, name: &str) -> CloudResult<String> {
        let url = format!("{ITEMS}/{folder_id}:/{}:/createUploadSession", utf8_percent_encode(name, SEGMENT));
        let body = json!({"item": {"@microsoft.graph.conflictBehavior": "replace"}});
        let mut answer = Vec::new();
        let r = self.session.call(&|t| Request::new(Method::Post, url.clone()).bearer(t).json(&body), &mut answer)?;
        if !r.ok() {
            return Err(status_error(&r));
        }
        Ok(parse(&answer)?["uploadUrl"].as_str().unwrap_or_default().to_owned())
    }
}

impl CloudProvider for OneDrive {
    fn ensure_folder(&self, path: &[String]) -> CloudResult<String> {
        let mut parent: Option<String> = None;
        for depth in 1..=path.len() {
            let key = path[..depth].to_vec();
            if let Some(id) = self.folders.lock().unwrap().get(&key) {
                parent = Some(id.clone());
                continue;
            }
            let id = match self.get(&format!("{}?select=id", by_path(&key)))? {
                Some(item) => item["id"].as_str().unwrap_or_default().to_owned(),
                None => {
                    // by id: Graph answers 400 invalidRequest to a POST on `special/approot/children` (live,
                    // 2026-11-21), while reading and uploading by an approot path work
                    let parent_id = match &parent {
                        Some(p) => p.clone(),
                        None => self.get(&format!("{APPROOT}?select=id"))?.ok_or(CloudError::NotFound)?["id"]
                            .as_str()
                            .unwrap_or_default()
                            .to_owned(),
                    };
                    let url = format!("{ITEMS}/{parent_id}/children");
                    let body = json!({"name": key[depth - 1], "folder": {}, "@microsoft.graph.conflictBehavior": "fail"});
                    let mut answer = Vec::new();
                    let r = self.session.call(&|t| Request::new(Method::Post, url.clone()).bearer(t).json(&body), &mut answer)?;
                    match r.status {
                        200..=299 => parse(&answer)?["id"].as_str().unwrap_or_default().to_owned(),
                        // another PC made it a moment ago
                        409 => self.get(&format!("{}?select=id", by_path(&key)))?.ok_or(CloudError::NotFound)?["id"]
                            .as_str()
                            .unwrap_or_default()
                            .to_owned(),
                        _ => return Err(status_error(&r)),
                    }
                }
            };
            self.folders.lock().unwrap().insert(key, id.clone());
            parent = Some(id);
        }
        parent.ok_or_else(|| CloudError::Permanent("empty path".into()))
    }

    fn read_small(&self, path: &[String]) -> CloudResult<Option<(Vec<u8>, Revision)>> {
        let Some((url, item)) = self.download_url(&by_path(path))? else { return Ok(None) };
        let mut body = Vec::new();
        let r = self.plain(Request::new(Method::Get, url), &mut body)?;
        if !r.ok() {
            return Err(status_error(&r));
        }
        Ok(Some((body, Revision(item["eTag"].as_str().unwrap_or_default().to_owned()))))
    }

    fn write_small(&self, path: &[String], bytes: &[u8], expected: Option<&Revision>) -> CloudResult<WriteOutcome> {
        if let Some((_, dir)) = path.split_last() {
            if !dir.is_empty() {
                self.ensure_folder(dir)?;
            }
        }
        let url = match expected {
            Some(_) => format!("{}/content", by_path(path)),
            None => format!("{}/content?@microsoft.graph.conflictBehavior=fail", by_path(path)),
        };
        let tag = expected.map(|r| r.0.clone());
        let mut answer = Vec::new();
        let r = self.session.call(
            &|t| {
                let request = Request::new(Method::Put, url.clone()).bearer(t).bytes("application/json", bytes.to_vec());
                match &tag {
                    Some(tag) => request.header("If-Match", tag.clone()),
                    None => request,
                }
            },
            &mut answer,
        )?;
        match r.status {
            200..=299 => Ok(WriteOutcome::Written(Revision(parse(&answer)?["eTag"].as_str().unwrap_or_default().to_owned()))),
            409 | 412 => Ok(WriteOutcome::Conflict),
            _ => Err(status_error(&r)),
        }
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
        // resume where Graph says it stands, or open a new session (an old one expires after a few days)
        let mut resumed = None;
        if let Some(s) = session.filter(|s| !s.url.is_empty()) {
            let mut body = Vec::new();
            let r = self.plain(Request::new(Method::Get, s.url.clone()), &mut body)?;
            if r.ok() {
                resumed = next_expected(&parse(&body)?).map(|offset| (s.url.clone(), offset));
            }
        }
        let (url, mut offset) = match resumed {
            Some(r) => r,
            None => (self.new_session(folder_id, name)?, 0),
        };
        on_session(UploadSession { url: url.clone(), offset });
        loop {
            let len = self.chunk.min(size - offset);
            source.seek(SeekFrom::Start(offset)).map_err(|e| CloudError::Permanent(e.to_string()))?;
            let range = format!("bytes {offset}-{}/{size}", offset + len.max(1) - 1);
            let mut answer = Vec::new();
            let r = self.plain(Request::new(Method::Put, url.clone()).header("Content-Range", range).stream(source, len), &mut answer)?;
            match r.status {
                202 => {
                    offset = next_expected(&parse(&answer)?).unwrap_or(offset + len);
                    on_session(UploadSession { url: url.clone(), offset });
                    if !progress(offset) {
                        return Err(CloudError::Cancelled);
                    }
                }
                200 | 201 => {
                    progress(size);
                    let item = parse(&answer)?;
                    let id = item["id"].as_str().unwrap_or_default().to_owned();
                    if sidecar::tagged(props) {
                        let note = sidecar::Note { path: id.clone(), name: name.to_owned(), size, props: props.clone() };
                        self.put_small(&Self::note_path(&id), &serde_json::to_vec(&note).unwrap_or_default())?;
                    }
                    return Ok(RemoteFile {
                        id,
                        name: name.to_owned(),
                        size,
                        hash: item["file"]["hashes"]["sha256Hash"].as_str().map(str::to_owned),
                        props: props.clone(),
                    });
                }
                _ => return Err(status_error(&r)),
            }
        }
    }

    fn download(&self, file_id: &str, sink: &mut dyn Write, progress: Progress) -> CloudResult<()> {
        let (url, _) = self.download_url(&format!("{ITEMS}/{file_id}"))?.ok_or(CloudError::NotFound)?;
        let mut counting = Counting { inner: sink, done: 0, progress };
        let r = self.plain(Request::new(Method::Get, url), &mut counting)?;
        if r.ok() {
            Ok(())
        } else {
            Err(status_error(&r))
        }
    }

    fn delete(&self, file_id: &str) -> CloudResult<()> {
        let url = format!("{ITEMS}/{file_id}");
        let r = self.session.call(&|t| Request::new(Method::Delete, url.clone()).bearer(t), &mut std::io::sink())?;
        if !r.ok() {
            return Err(status_error(&r));
        }
        let note = by_path(&Self::note_path(file_id));
        let _ = self.session.call(&|t| Request::new(Method::Delete, note.clone()).bearer(t), &mut std::io::sink());
        Ok(())
    }

    fn list_tagged(&self, _root_id: &str) -> CloudResult<Vec<RemoteFile>> {
        let mut next = Some(format!("{}/children?select=name,@microsoft.graph.downloadUrl", by_path(&[NOTES.to_owned()])));
        let mut urls = Vec::new();
        while let Some(page_url) = next.take() {
            let Some(page) = self.get(&page_url)? else { return Ok(Vec::new()) };
            urls.extend(page["value"].as_array().into_iter().flatten().filter_map(|v| v["@microsoft.graph.downloadUrl"].as_str().map(str::to_owned)));
            next = page["@odata.nextLink"].as_str().map(str::to_owned);
        }
        let mut files = Vec::new();
        for url in urls {
            let mut bytes = Vec::new();
            if !self.plain(Request::new(Method::Get, url), &mut bytes)?.ok() {
                continue;
            }
            let Ok(note) = serde_json::from_slice::<sidecar::Note>(&bytes) else { continue };
            files.push(RemoteFile { id: note.path, name: note.name, size: note.size, hash: None, props: note.props });
        }
        Ok(files)
    }

    fn space(&self) -> CloudResult<ProviderSpace> {
        // app-folder access may not read the drive itself (live, 2026-11-21: 403 accessDenied)
        let drive = match self.get("https://graph.microsoft.com/v1.0/me/drive?select=quota") {
            Err(CloudError::Permanent(e)) if e.starts_with("HTTP 403") => return Ok(ProviderSpace::UNKNOWN),
            other => other?.ok_or(CloudError::NotFound)?,
        };
        let quota = &drive["quota"];
        Ok(match (quota["remaining"].as_u64(), quota["total"].as_u64()) {
            (Some(free), total) => ProviderSpace { free, total },
            _ => ProviderSpace { free: u64::MAX, total: None },
        })
    }

    fn stream(&self, file_id: &str) -> CloudResult<StreamSource> {
        // a pre-signed URL (about an hour): the player needs no header
        let (url, _) = self.download_url(&format!("{ITEMS}/{file_id}"))?.ok_or(CloudError::NotFound)?;
        Ok(StreamSource { url, headers: Vec::new() })
    }

    fn web_link(&self, _file_id: &str) -> Option<String> {
        Some("https://onedrive.live.com/".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::fake::FakeHttp;
    use crate::oauth::{ClientConfig, Tokens, MICROSOFT};
    use chrono::{Duration, Utc};
    use std::io::Cursor;
    use std::sync::Arc;

    fn onedrive(http: &Arc<FakeHttp>) -> OneDrive {
        let tokens = Tokens {
            access_token: "at".into(),
            refresh_token: "rt".into(),
            expires_at: Utc::now() + Duration::hours(1),
            sub: "ms".into(),
            email: String::new(),
        };
        OneDrive::new(OAuthSession::new(http.clone(), &MICROSOFT, ClientConfig { id: "c".into(), secret: String::new() }, tokens, |_| {}))
    }

    fn ok(v: Value) -> (u16, Vec<(&'static str, String)>, Vec<u8>) {
        (200, vec![], v.to_string().into_bytes())
    }

    #[test]
    fn folders_are_found_or_made_in_the_app_folder() {
        let http = Arc::new(FakeHttp::default());
        let d = onedrive(&http);
        // the top folder is new: made under the app folder's id, never on `special/approot/children`
        http.expect(|r| {
            assert!(r.url.starts_with(&format!("{APPROOT}:/Lag:")), "{}", r.url);
            (404, vec![], vec![])
        });
        http.expect(|r| {
            assert!(r.url.starts_with(&format!("{APPROOT}?select=id")), "{}", r.url);
            ok(json!({"id": "ROOT!s1"}))
        });
        http.expect(|r| {
            assert_eq!((r.method, r.url.as_str()), (Method::Post, format!("{ITEMS}/ROOT!s1/children").as_str()));
            (201, vec![], json!({"id": "LAG"}).to_string().into_bytes())
        });
        http.expect(|r| {
            assert!(r.url.contains(":/Lag/Counter-Strike%202:"), "{}", r.url);
            (404, vec![], vec![])
        });
        http.expect(|r| {
            assert_eq!((r.method, r.url.as_str()), (Method::Post, format!("{ITEMS}/LAG/children").as_str()));
            assert_eq!(r.json()["name"], "Counter-Strike 2");
            (201, vec![], json!({"id": "CS2"}).to_string().into_bytes())
        });
        let path = vec!["Lag".to_string(), "Counter-Strike 2".to_string()];
        assert_eq!(d.ensure_folder(&path).unwrap(), "CS2");
        assert_eq!(d.ensure_folder(&path).unwrap(), "CS2");
        assert_eq!(http.remaining(), 0, "cached");
    }

    #[test]
    fn the_index_with_etags_and_signed_downloads() {
        let http = Arc::new(FakeHttp::default());
        let d = onedrive(&http);
        let index: Vec<String> = ["Lag", ".lag", "library.json"].iter().map(|s| s.to_string()).collect();
        http.expect(|_| ok(json!({"id": "LAG"})));
        http.expect(|_| ok(json!({"id": "SYS"})));
        http.expect(|r| {
            assert!(r.url.ends_with("/content?@microsoft.graph.conflictBehavior=fail"));
            (201, vec![], json!({"eTag": "e1"}).to_string().into_bytes())
        });
        assert_eq!(d.write_small(&index, b"{}", None).unwrap(), WriteOutcome::Written(Revision("e1".into())));
        http.expect(|r| {
            assert_eq!(r.header("If-Match"), Some("e1"));
            (412, vec![], vec![])
        });
        assert_eq!(d.write_small(&index, b"{}", Some(&Revision("e1".into()))).unwrap(), WriteOutcome::Conflict);
        http.expect(|_| ok(json!({"id": "IDX", "eTag": "e2", "@microsoft.graph.downloadUrl": "https://dl.example/idx"})));
        http.expect(|r| {
            assert_eq!(r.url, "https://dl.example/idx");
            assert_eq!(r.header("Authorization"), None, "no token to the storage host");
            (200, vec![], b"{\"a\":1}".to_vec())
        });
        assert_eq!(d.read_small(&index).unwrap(), Some((b"{\"a\":1}".to_vec(), Revision("e2".into()))));
    }

    #[test]
    fn uploads_resume_from_the_range_graph_expects() {
        let http = Arc::new(FakeHttp::default());
        let d = onedrive(&http).with_chunk(4);
        let data = b"0123456789".to_vec();
        let mut props = FileProps::new();
        props.insert(props::CLIP_ID.into(), "c1".into());
        http.expect(|r| {
            assert!(r.url.ends_with("/items/CS2:/a%20b.mp4:/createUploadSession"), "{}", r.url);
            ok(json!({"uploadUrl": "https://up.example/s1"}))
        });
        http.expect(|r| {
            assert_eq!((r.header("Content-Range"), r.body.as_slice()), (Some("bytes 0-3/10"), &b"0123"[..]));
            assert_eq!(r.header("Authorization"), None);
            (202, vec![], json!({"nextExpectedRanges": ["4-"]}).to_string().into_bytes())
        });
        http.expect(|_| (503, vec![], vec![]));
        let mut sessions = Vec::new();
        let err = d.upload("CS2", "a b.mp4", 10, &mut Cursor::new(&data), &props, None, &mut |s| sessions.push(s), &mut |_| true).unwrap_err();
        assert!(err.is_retryable());
        let stored = sessions.last().unwrap().clone();
        assert_eq!(stored.offset, 4);

        http.expect(|r| {
            assert_eq!((r.method, r.url.as_str()), (Method::Get, "https://up.example/s1"));
            ok(json!({"nextExpectedRanges": ["6-9"]}))
        });
        http.expect(|r| {
            assert_eq!((r.header("Content-Range"), r.body.as_slice()), (Some("bytes 6-9/10"), &b"6789"[..]));
            (201, vec![], json!({"id": "F1", "file": {"hashes": {"sha256Hash": "H"}}}).to_string().into_bytes())
        });
        http.expect(|r| {
            assert!(r.url.contains(":/.lag-props/") && r.url.ends_with(".json:/content"), "{}", r.url);
            let note: sidecar::Note = serde_json::from_slice(&r.body).unwrap();
            assert_eq!((note.path.as_str(), note.name.as_str()), ("F1", "a b.mp4"));
            (201, vec![], b"{}".to_vec())
        });
        let file = d.upload("CS2", "a b.mp4", 10, &mut Cursor::new(&data), &props, Some(&stored), &mut |_| {}, &mut |_| true).unwrap();
        assert_eq!((file.id.as_str(), file.hash.as_deref()), ("F1", Some("H")));
        assert_eq!(http.remaining(), 0);
    }

    #[test]
    fn space_listing_and_errors() {
        let http = Arc::new(FakeHttp::default());
        let d = onedrive(&http);
        http.expect(|_| ok(json!({"quota": {"remaining": 4000, "total": 5000}})));
        assert_eq!(d.space().unwrap(), ProviderSpace { free: 4000, total: Some(5000) });
        http.expect(|_| (403, vec![], br#"{"error":{"code":"accessDenied","message":"Access denied"}}"#.to_vec()));
        assert!(d.space().unwrap().is_unknown(), "app-folder access cannot read the quota: unknown, not an error");
        http.expect(|_| ok(json!({"value": [{"name": "x.json", "@microsoft.graph.downloadUrl": "https://dl.example/n"}]})));
        let mut p = FileProps::new();
        p.insert(props::CLIP_ID.into(), "c".into());
        let note = serde_json::to_vec(&sidecar::Note { path: "F1".into(), name: "a.mp4".into(), size: 7, props: p }).unwrap();
        http.expect(move |_| (200, vec![], note));
        let files = d.list_tagged("ignored").unwrap();
        assert_eq!((files[0].id.as_str(), files[0].name.as_str(), files[0].size), ("F1", "a.mp4", 7));
        http.expect(|_| (507, vec![], br#"{"error":{"code":"quotaLimitReached"}}"#.to_vec()));
        let err = d.upload("CS2", "b.mp4", 1, &mut Cursor::new(b"x"), &FileProps::new(), None, &mut |_| {}, &mut |_| true).unwrap_err();
        assert_eq!(err, CloudError::Full);
    }
}
