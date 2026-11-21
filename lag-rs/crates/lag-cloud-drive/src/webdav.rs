//! WebDAV as a cloud: Nextcloud, ownCloud, Synology, QNAP, TrueNAS, Seafile and any server speaking WebDAV
//! (RFC 4918). Folders by MKCOL, files by PUT / GET / DELETE; the index is written with `If-Match` on its ETag
//! (`If-None-Match: *` when new), so two PCs never overwrite each other; the quota comes from RFC 4331
//! properties. Lag's file properties are notes in `.lag/props` (sidecar.rs). Login + app password over Basic
//! auth (HTTPS); the password is kept encrypted by the app, never in settings.
use crate::http::{HttpClient, Method, Request, Response};
use crate::sidecar;
use base64::Engine;
use lag_cloud::provider::*;
use lag_cloud::queue::UploadSession;
use lag_cloud::quota::ProviderSpace;
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use std::collections::HashSet;
use std::io::{Read, SeekFrom, Write};
use std::sync::{Arc, Mutex};

/// A path segment, percent-encoded (spaces, Unicode, '#', '?'...).
const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');

pub struct WebDav {
    http: Arc<dyn HttpClient>,
    /// Ends with '/': e.g. https://cloud.example.com/remote.php/dav/files/me/
    base: String,
    auth: String,
    made: Mutex<HashSet<String>>,
}

fn status_error(r: &Response) -> CloudError {
    match r.status {
        401 | 403 => CloudError::SignIn,
        404 | 410 => CloudError::NotFound,
        507 | 413 => CloudError::Full,
        408 | 423 | 429 | 500..=599 => CloudError::Transient(format!("HTTP {}", r.status)),
        s => CloudError::Permanent(format!("HTTP {s}")),
    }
}

fn transport(e: std::io::Error) -> CloudError {
    if e.kind() == std::io::ErrorKind::Interrupted {
        CloudError::Cancelled
    } else {
        CloudError::Transient(e.to_string())
    }
}

/// Reads through to the upload and reports how far it got; stops the upload when told to.
struct Counted<'a> {
    inner: &'a mut dyn ReadSeek,
    done: u64,
    reported: u64,
    progress: Progress<'a>,
}

impl Read for Counted<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.done += n as u64;
        if self.done - self.reported >= 1 << 20 || n == 0 {
            self.reported = self.done;
            if !(self.progress)(self.done) {
                return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled"));
            }
        }
        Ok(n)
    }
}

impl WebDav {
    /// `base_url`: the user's WebDAV folder (Nextcloud: https://host/remote.php/dav/files/<user>/).
    pub fn new(http: Arc<dyn HttpClient>, base_url: &str, user: &str, password: &str) -> Self {
        let mut base = base_url.trim().to_owned();
        if !base.ends_with('/') {
            base.push('/');
        }
        let token = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"));
        Self { http, base, auth: format!("Basic {token}"), made: Mutex::default() }
    }

    fn url(&self, path: &[String]) -> String {
        let encoded: Vec<String> = path.iter().map(|s| utf8_percent_encode(s, SEGMENT).to_string()).collect();
        format!("{}{}", self.base, encoded.join("/"))
    }

    fn request<'a>(&self, method: Method, url: &str) -> Request<'a> {
        Request::new(method, url).header("Authorization", self.auth.clone())
    }

    fn send(&self, request: Request, sink: &mut dyn Write) -> CloudResult<Response> {
        self.http.send(request, sink).map_err(transport)
    }

    /// The path below the base of a URL this provider made.
    fn relative(&self, url: &str) -> String {
        let rest = url.strip_prefix(&self.base).unwrap_or(url);
        rest.split('/')
            .map(|s| percent_encoding::percent_decode_str(s).decode_utf8_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/")
    }

    /// `<root>/.lag/props/` for a file under `<root>/`.
    fn notes_url(&self, file_url: &str) -> String {
        let root = self.relative(file_url).split('/').next().unwrap_or("").to_owned();
        let mut path = vec![root];
        path.extend(sidecar::DIR.iter().map(|s| s.to_string()));
        format!("{}/", self.url(&path))
    }

    /// Checks the address and the login (Settings > Connect).
    pub fn check(&self) -> CloudResult<()> {
        let r = self.send(self.request(Method::Propfind, &self.base).header("Depth", "0"), &mut std::io::sink())?;
        match r.status {
            200..=299 => Ok(()),
            _ => Err(status_error(&r)),
        }
    }

    fn etag(r: &Response) -> Option<Revision> {
        r.header("etag").map(|e| Revision(e.to_owned()))
    }

    fn put_small(&self, url: &str, bytes: &[u8]) -> CloudResult<()> {
        let r = self.send(self.request(Method::Put, url).bytes("application/json", bytes.to_vec()), &mut std::io::sink())?;
        if r.ok() {
            Ok(())
        } else {
            Err(status_error(&r))
        }
    }

    /// Hrefs listed by a Depth 1 PROPFIND of a folder (the folder itself excluded).
    fn list(&self, folder_url: &str) -> CloudResult<Vec<String>> {
        let mut body = Vec::new();
        let r = self.send(self.request(Method::Propfind, folder_url).header("Depth", "1"), &mut body)?;
        match r.status {
            200..=299 => {}
            404 => return Ok(Vec::new()),
            _ => return Err(status_error(&r)),
        }
        let text = String::from_utf8_lossy(&body);
        let doc = roxmltree::Document::parse(&text).map_err(|e| CloudError::Permanent(format!("bad WebDAV answer: {e}")))?;
        Ok(doc
            .descendants()
            .filter(|n| n.tag_name().name() == "href")
            .filter_map(|n| n.text().map(str::trim).map(str::to_owned))
            .filter(|h| !h.ends_with('/'))
            .collect())
    }

    /// An href from a PROPFIND (absolute path) as a full URL.
    fn absolute(&self, href: &str) -> String {
        if href.starts_with("http://") || href.starts_with("https://") {
            return href.to_owned();
        }
        let origin_end = self.base.find("://").map(|i| i + 3).and_then(|i| self.base[i..].find('/').map(|j| i + j));
        format!("{}{}", &self.base[..origin_end.unwrap_or(self.base.len())], href)
    }
}

impl CloudProvider for WebDav {
    fn ensure_folder(&self, path: &[String]) -> CloudResult<String> {
        for depth in 1..=path.len() {
            let url = format!("{}/", self.url(&path[..depth]));
            if self.made.lock().unwrap().contains(&url) {
                continue;
            }
            let r = self.send(self.request(Method::Mkcol, &url), &mut std::io::sink())?;
            // 405: it already exists
            if !(r.ok() || r.status == 405) {
                return Err(status_error(&r));
            }
            self.made.lock().unwrap().insert(url);
        }
        Ok(format!("{}/", self.url(path)))
    }

    fn read_small(&self, path: &[String]) -> CloudResult<Option<(Vec<u8>, Revision)>> {
        let mut body = Vec::new();
        let r = self.send(self.request(Method::Get, &self.url(path)), &mut body)?;
        match r.status {
            200 => Ok(Some((body, Self::etag(&r).unwrap_or(Revision(String::new()))))),
            404 => Ok(None),
            _ => Err(status_error(&r)),
        }
    }

    fn write_small(&self, path: &[String], bytes: &[u8], expected: Option<&Revision>) -> CloudResult<WriteOutcome> {
        if let Some((_, dir)) = path.split_last() {
            self.ensure_folder(dir)?;
        }
        let url = self.url(path);
        let request = self.request(Method::Put, &url).bytes("application/json", bytes.to_vec());
        let request = match expected {
            Some(rev) => request.header("If-Match", rev.0.clone()),
            None => request.header("If-None-Match", "*"),
        };
        let r = self.send(request, &mut std::io::sink())?;
        match r.status {
            200..=299 => {}
            412 => return Ok(WriteOutcome::Conflict),
            _ => return Err(status_error(&r)),
        }
        // the new ETag (some servers answer the PUT without one)
        let rev = match Self::etag(&r) {
            Some(rev) => rev,
            None => self.read_small(path)?.map(|(_, rev)| rev).unwrap_or(Revision(String::new())),
        };
        Ok(WriteOutcome::Written(rev))
    }

    fn upload(
        &self,
        folder_id: &str,
        name: &str,
        size: u64,
        source: &mut dyn ReadSeek,
        props: &FileProps,
        _session: Option<&UploadSession>,
        on_session: &mut dyn FnMut(UploadSession),
        progress: Progress,
    ) -> CloudResult<RemoteFile> {
        // plain WebDAV has no resumable upload: a stopped upload starts over
        let url = format!("{folder_id}{}", utf8_percent_encode(name, SEGMENT));
        source.seek(SeekFrom::Start(0)).map_err(|e| CloudError::Permanent(e.to_string()))?;
        on_session(UploadSession { url: url.clone(), offset: 0 });
        let mut counted = Counted { inner: source, done: 0, reported: 0, progress };
        let request = self.request(Method::Put, &url).header("Content-Type", "application/octet-stream").stream(&mut counted, size);
        let r = self.send(request, &mut std::io::sink())?;
        if !r.ok() {
            return Err(status_error(&r));
        }
        (counted.progress)(size);
        if sidecar::tagged(props) {
            let rel = self.relative(&url);
            let notes = self.notes_url(&url);
            let segments: Vec<String> = self.relative(&notes).trim_end_matches('/').split('/').map(str::to_owned).collect();
            self.ensure_folder(&segments)?;
            let note = sidecar::Note { path: rel.clone(), size, props: props.clone() };
            self.put_small(&format!("{notes}{}", sidecar::name(&rel)), &serde_json::to_vec(&note).unwrap_or_default())?;
        }
        Ok(RemoteFile { id: url, name: name.to_owned(), size, hash: None, props: props.clone() })
    }

    fn download(&self, file_id: &str, sink: &mut dyn Write, progress: Progress) -> CloudResult<()> {
        struct Counting<'a> {
            inner: &'a mut dyn Write,
            done: u64,
            progress: Progress<'a>,
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
        let mut counting = Counting { inner: sink, done: 0, progress };
        let r = self.send(self.request(Method::Get, file_id), &mut counting)?;
        if r.ok() {
            Ok(())
        } else {
            Err(status_error(&r))
        }
    }

    fn delete(&self, file_id: &str) -> CloudResult<()> {
        let r = self.send(self.request(Method::Delete, file_id), &mut std::io::sink())?;
        if !r.ok() {
            return Err(status_error(&r));
        }
        let note = format!("{}{}", self.notes_url(file_id), sidecar::name(&self.relative(file_id)));
        let _ = self.send(self.request(Method::Delete, &note), &mut std::io::sink());
        Ok(())
    }

    fn list_tagged(&self, root_id: &str) -> CloudResult<Vec<RemoteFile>> {
        let notes = format!("{root_id}{}/", sidecar::DIR.join("/"));
        let mut files = Vec::new();
        for href in self.list(&notes)? {
            let Ok(Some((bytes, _))) = self.read_small_url(&self.absolute(&href)) else { continue };
            let Ok(note) = serde_json::from_slice::<sidecar::Note>(&bytes) else { continue };
            let segments: Vec<String> = note.path.split('/').map(str::to_owned).collect();
            files.push(RemoteFile {
                id: self.url(&segments),
                name: segments.last().cloned().unwrap_or_default(),
                size: note.size,
                hash: None,
                props: note.props,
            });
        }
        Ok(files)
    }

    fn space(&self) -> CloudResult<ProviderSpace> {
        let body = r#"<?xml version="1.0"?><d:propfind xmlns:d="DAV:"><d:prop><d:quota-available-bytes/><d:quota-used-bytes/></d:prop></d:propfind>"#;
        let mut answer = Vec::new();
        let r = self.send(
            self.request(Method::Propfind, &self.base).header("Depth", "0").bytes("application/xml", body.as_bytes().to_vec()),
            &mut answer,
        )?;
        if !r.ok() {
            return Err(status_error(&r));
        }
        let text = String::from_utf8_lossy(&answer);
        let doc = roxmltree::Document::parse(&text).map_err(|e| CloudError::Permanent(format!("bad WebDAV answer: {e}")))?;
        let number = |name: &str| {
            doc.descendants().find(|n| n.tag_name().name() == name).and_then(|n| n.text()).and_then(|t| t.trim().parse::<i64>().ok())
        };
        // RFC 4331: a negative or missing available amount means "unknown / unlimited"
        Ok(match (number("quota-available-bytes"), number("quota-used-bytes")) {
            (Some(free), used) if free >= 0 => {
                ProviderSpace { free: free as u64, total: used.filter(|u| *u >= 0).map(|u| (u + free) as u64) }
            }
            _ => ProviderSpace { free: u64::MAX, total: None },
        })
    }

    fn stream(&self, file_id: &str) -> CloudResult<StreamSource> {
        Ok(StreamSource { url: file_id.to_owned(), headers: vec![("Authorization".into(), self.auth.clone())] })
    }

    fn web_link(&self, _file_id: &str) -> Option<String> {
        None
    }
}

impl WebDav {
    fn read_small_url(&self, url: &str) -> CloudResult<Option<(Vec<u8>, Revision)>> {
        let mut body = Vec::new();
        let r = self.send(self.request(Method::Get, url), &mut body)?;
        match r.status {
            200 => Ok(Some((body, Self::etag(&r).unwrap_or(Revision(String::new()))))),
            404 => Ok(None),
            _ => Err(status_error(&r)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::fake::FakeHttp;
    use std::io::Cursor;

    const BASE: &str = "https://cloud.example.com/remote.php/dav/files/me";

    fn dav(http: &Arc<FakeHttp>) -> WebDav {
        WebDav::new(http.clone(), BASE, "me", "app-password")
    }

    #[test]
    fn folders_are_made_once_and_existing_ones_are_fine() {
        let http = Arc::new(FakeHttp::default());
        let d = dav(&http);
        http.expect(|r| {
            assert_eq!((r.method, r.url.as_str()), (Method::Mkcol, "https://cloud.example.com/remote.php/dav/files/me/Lag/"));
            assert!(r.header("Authorization").unwrap().starts_with("Basic "));
            (405, vec![], vec![])
        });
        http.expect(|r| {
            assert!(r.url.ends_with("/Lag/Counter-Strike%202/"), "{}", r.url);
            (201, vec![], vec![])
        });
        let path = vec!["Lag".to_string(), "Counter-Strike 2".to_string()];
        assert_eq!(d.ensure_folder(&path).unwrap(), format!("{BASE}/Lag/Counter-Strike%202/"));
        d.ensure_folder(&path).unwrap();
        assert_eq!(http.remaining(), 0, "cached");
    }

    #[test]
    fn the_index_uses_etags_so_two_pcs_never_overwrite_each_other() {
        let http = Arc::new(FakeHttp::default());
        let d = dav(&http);
        let path: Vec<String> = ["Lag", ".lag", "library.json"].iter().map(|s| s.to_string()).collect();
        http.expect(|_| (405, vec![], vec![]));
        http.expect(|_| (405, vec![], vec![]));
        http.expect(|r| {
            assert_eq!(r.header("If-None-Match"), Some("*"), "new file: only if nobody made it first");
            (201, vec![("ETag", "\"v1\"".into())], vec![])
        });
        assert_eq!(d.write_small(&path, b"{}", None).unwrap(), WriteOutcome::Written(Revision("\"v1\"".into())));
        http.expect(|r| {
            assert_eq!(r.header("If-Match"), Some("\"v1\""));
            (412, vec![], vec![])
        });
        assert_eq!(d.write_small(&path, b"{}", Some(&Revision("\"v1\"".into()))).unwrap(), WriteOutcome::Conflict);
        http.expect(|_| (200, vec![("ETag", "\"v2\"".into())], b"{\"a\":1}".to_vec()));
        assert_eq!(d.read_small(&path).unwrap(), Some((b"{\"a\":1}".to_vec(), Revision("\"v2\"".into()))));
        http.expect(|_| (404, vec![], vec![]));
        assert_eq!(d.read_small(&path).unwrap(), None);
    }

    #[test]
    fn uploads_leave_a_note_and_listing_reads_the_notes() {
        let http = Arc::new(FakeHttp::default());
        let d = dav(&http);
        let folder = format!("{BASE}/Lag/Desktop/");
        let mut props = FileProps::new();
        props.insert(props::CLIP_ID.into(), "c1".into());
        http.expect(|r| {
            assert_eq!(r.method, Method::Put);
            assert!(r.url.ends_with("/Lag/Desktop/my%20clip.mp4"));
            assert_eq!(r.body, b"0123456789");
            (201, vec![], vec![])
        });
        http.expect(|_| (405, vec![], vec![]));
        http.expect(|_| (201, vec![], vec![]));
        http.expect(|_| (201, vec![], vec![]));
        http.expect(|r| {
            assert!(r.url.contains("/Lag/.lag/props/") && r.url.ends_with(".json"));
            let note: sidecar::Note = serde_json::from_slice(&r.body).unwrap();
            assert_eq!((note.path.as_str(), note.size), ("Lag/Desktop/my clip.mp4", 10));
            (201, vec![], vec![])
        });
        let mut seen = 0;
        let file = d
            .upload(&folder, "my clip.mp4", 10, &mut Cursor::new(b"0123456789"), &props, None, &mut |_| {}, &mut |n| {
                seen = n;
                true
            })
            .unwrap();
        assert_eq!((file.size, seen), (10, 10));
        assert_eq!(http.remaining(), 0);

        // listing: PROPFIND the notes, GET each
        let note = serde_json::to_vec(&sidecar::Note { path: "Lag/Desktop/my clip.mp4".into(), size: 10, props: props.clone() }).unwrap();
        http.expect(|r| {
            assert_eq!((r.method, r.header("Depth")), (Method::Propfind, Some("1")));
            let xml = r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:">
                <d:response><d:href>/remote.php/dav/files/me/Lag/.lag/props/</d:href></d:response>
                <d:response><d:href>/remote.php/dav/files/me/Lag/.lag/props/abc.json</d:href></d:response>
              </d:multistatus>"#;
            (207, vec![], xml.as_bytes().to_vec())
        });
        http.expect(move |r| {
            assert_eq!(r.url, "https://cloud.example.com/remote.php/dav/files/me/Lag/.lag/props/abc.json");
            (200, vec![], note)
        });
        let files = d.list_tagged(&format!("{BASE}/Lag/")).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].id, file.id);
        assert_eq!(files[0].props[props::CLIP_ID], "c1");
    }

    #[test]
    fn quota_login_and_full_server() {
        let http = Arc::new(FakeHttp::default());
        let d = dav(&http);
        http.expect(|_| {
            let xml = r#"<d:multistatus xmlns:d="DAV:"><d:response><d:propstat><d:prop>
                <d:quota-available-bytes>4000</d:quota-available-bytes><d:quota-used-bytes>1000</d:quota-used-bytes>
              </d:prop></d:propstat></d:response></d:multistatus>"#;
            (207, vec![], xml.as_bytes().to_vec())
        });
        assert_eq!(d.space().unwrap(), ProviderSpace { free: 4000, total: Some(5000) });
        http.expect(|_| (207, vec![], br#"<d:multistatus xmlns:d="DAV:"/>"#.to_vec()));
        assert_eq!(d.space().unwrap(), ProviderSpace { free: u64::MAX, total: None }, "no quota reported: unlimited");
        http.expect(|_| (401, vec![], vec![]));
        assert_eq!(d.check().unwrap_err(), CloudError::SignIn, "wrong password");
        http.expect(|_| (507, vec![], vec![]));
        let err = d
            .upload(&format!("{BASE}/Lag/"), "a.mp4", 1, &mut Cursor::new(b"x"), &FileProps::new(), None, &mut |_| {}, &mut |_| true)
            .unwrap_err();
        assert_eq!(err, CloudError::Full);
        assert!(d.stream(&format!("{BASE}/a.mp4")).unwrap().headers[0].1.starts_with("Basic "));
    }
}
