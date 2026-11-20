//! A cloud that lives in memory: the provider every test (and a future offline UI preview) runs against. It
//! behaves like the real ones where it matters: folders by path, revisions on small files, resumable uploads
//! in chunks, limited space, and faults injected on purpose (a dropped connection mid-upload, a full cloud,
//! an expired sign-in, another PC writing the index at the same moment).
use crate::provider::*;
use crate::queue::UploadSession;
use crate::quota::ProviderSpace;
use std::collections::{BTreeMap, VecDeque};
use std::io::{SeekFrom, Write};
use std::sync::Mutex;

/// Upload chunk size of the fake (real providers use 8-16 MB).
pub const CHUNK: usize = 4;

#[derive(Debug, Clone)]
struct MemFile {
    folder: String,
    name: String,
    bytes: Vec<u8>,
    props: FileProps,
}

#[derive(Default)]
struct State {
    next_id: u64,
    /// folder path -> id
    folders: BTreeMap<Vec<String>, String>,
    files: BTreeMap<String, MemFile>,
    /// small files by path: bytes + revision counter
    small: BTreeMap<Vec<String>, (Vec<u8>, u64)>,
    /// open upload sessions: url -> bytes received so far
    sessions: BTreeMap<String, Vec<u8>>,
    capacity: u64,
    faults: VecDeque<Fault>,
    /// before the next write_small: another PC writes these bytes first (a race)
    racing_writes: VecDeque<Vec<u8>>,
}

/// A fault the next matching call meets.
#[derive(Debug, Clone, PartialEq)]
pub enum Fault {
    /// The connection drops after this many bytes of the next upload.
    DropUploadAfter(u64),
    /// The next call of any kind fails with this.
    Next(CloudError),
}

pub struct MemoryCloud {
    state: Mutex<State>,
}

impl MemoryCloud {
    pub fn new(capacity: u64) -> Self {
        Self { state: Mutex::new(State { capacity, ..Default::default() }) }
    }
    pub fn inject(&self, fault: Fault) {
        self.state.lock().unwrap().faults.push_back(fault);
    }
    /// Another PC writes `bytes` to the next small file written, right before us.
    pub fn race_next_write(&self, bytes: Vec<u8>) {
        self.state.lock().unwrap().racing_writes.push_back(bytes);
    }
    /// Names of the files in a folder (tests check the tree).
    pub fn file_names(&self, path: &[&str]) -> Vec<String> {
        let s = self.state.lock().unwrap();
        let key: Vec<String> = path.iter().map(|p| p.to_string()).collect();
        let Some(id) = s.folders.get(&key) else { return Vec::new() };
        s.files.values().filter(|f| &f.folder == id).map(|f| f.name.clone()).collect()
    }
    pub fn file_count(&self) -> usize {
        self.state.lock().unwrap().files.len()
    }
    pub fn file_bytes(&self, id: &str) -> Option<Vec<u8>> {
        self.state.lock().unwrap().files.get(id).map(|f| f.bytes.clone())
    }
    /// The user moves a file to another folder in the cloud's web UI.
    pub fn user_moves(&self, file_id: &str, to: &[&str]) {
        let to: Vec<String> = to.iter().map(|p| p.to_string()).collect();
        let folder = self.ensure_folder(&to).unwrap();
        if let Some(f) = self.state.lock().unwrap().files.get_mut(file_id) {
            f.folder = folder;
        }
    }

    fn take_fault(s: &mut State) -> CloudResult<()> {
        if let Some(Fault::Next(_)) = s.faults.front() {
            if let Some(Fault::Next(e)) = s.faults.pop_front() {
                return Err(e);
            }
        }
        Ok(())
    }
    fn used(s: &State) -> u64 {
        s.files.values().map(|f| f.bytes.len() as u64).sum()
    }
    fn new_id(s: &mut State, prefix: &str) -> String {
        s.next_id += 1;
        format!("{prefix}{}", s.next_id)
    }
}

impl CloudProvider for MemoryCloud {
    fn ensure_folder(&self, path: &[String]) -> CloudResult<String> {
        let mut s = self.state.lock().unwrap();
        Self::take_fault(&mut s)?;
        let mut id = String::new();
        for depth in 1..=path.len() {
            let key = path[..depth].to_vec();
            id = match s.folders.get(&key) {
                Some(id) => id.clone(),
                None => {
                    let id = Self::new_id(&mut s, "dir");
                    s.folders.insert(key, id.clone());
                    id
                }
            };
        }
        Ok(id)
    }

    fn read_small(&self, path: &[String]) -> CloudResult<Option<(Vec<u8>, Revision)>> {
        let mut s = self.state.lock().unwrap();
        Self::take_fault(&mut s)?;
        Ok(s.small.get(path).map(|(b, rev)| (b.clone(), Revision(rev.to_string()))))
    }

    fn write_small(&self, path: &[String], bytes: &[u8], expected: Option<&Revision>) -> CloudResult<WriteOutcome> {
        let mut s = self.state.lock().unwrap();
        Self::take_fault(&mut s)?;
        if let Some(theirs) = s.racing_writes.pop_front() {
            let rev = s.small.get(path).map_or(0, |(_, r)| *r) + 1;
            s.small.insert(path.to_vec(), (theirs, rev));
        }
        let current = s.small.get(path).map(|(_, r)| Revision(r.to_string()));
        if current.as_ref() != expected {
            return Ok(WriteOutcome::Conflict);
        }
        let rev = s.small.get(path).map_or(0, |(_, r)| *r) + 1;
        s.small.insert(path.to_vec(), (bytes.to_vec(), rev));
        Ok(WriteOutcome::Written(Revision(rev.to_string())))
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
        let (url, mut received, drop_after) = {
            let mut s = self.state.lock().unwrap();
            Self::take_fault(&mut s)?;
            if Self::used(&s) + size > s.capacity {
                return Err(CloudError::Full);
            }
            let drop_after = match s.faults.front() {
                Some(Fault::DropUploadAfter(n)) => {
                    let n = *n;
                    s.faults.pop_front();
                    Some(n)
                }
                _ => None,
            };
            let resumed = session.and_then(|sess| s.sessions.get(&sess.url).map(|b| (sess.url.clone(), b.clone())));
            let (url, received) = match resumed {
                Some(r) => r,
                None => {
                    let url = format!("mem://upload/{}", Self::new_id(&mut s, "s"));
                    s.sessions.insert(url.clone(), Vec::new());
                    (url, Vec::new())
                }
            };
            (url, received, drop_after)
        };
        on_session(UploadSession { url: url.clone(), offset: received.len() as u64 });
        source.seek(SeekFrom::Start(received.len() as u64)).map_err(|e| CloudError::Permanent(e.to_string()))?;
        let mut chunk = [0u8; CHUNK];
        let mut sent_now = 0u64;
        while (received.len() as u64) < size {
            if drop_after.is_some_and(|n| sent_now >= n) {
                self.state.lock().unwrap().sessions.insert(url.clone(), received.clone());
                return Err(CloudError::Transient("connection dropped".into()));
            }
            let want = CHUNK.min((size - received.len() as u64) as usize);
            source.read_exact(&mut chunk[..want]).map_err(|e| CloudError::Permanent(e.to_string()))?;
            received.extend_from_slice(&chunk[..want]);
            sent_now += want as u64;
            on_session(UploadSession { url: url.clone(), offset: received.len() as u64 });
            if !progress(received.len() as u64) {
                self.state.lock().unwrap().sessions.insert(url.clone(), received.clone());
                return Err(CloudError::Cancelled);
            }
        }
        let mut s = self.state.lock().unwrap();
        s.sessions.remove(&url);
        let id = Self::new_id(&mut s, "file");
        let file = MemFile { folder: folder_id.to_owned(), name: name.to_owned(), bytes: received, props: props.clone() };
        let remote = RemoteFile {
            id: id.clone(),
            name: file.name.clone(),
            size: file.bytes.len() as u64,
            hash: None,
            props: file.props.clone(),
        };
        s.files.insert(id, file);
        Ok(remote)
    }

    fn download(&self, file_id: &str, sink: &mut dyn Write, progress: Progress) -> CloudResult<()> {
        let bytes = {
            let mut s = self.state.lock().unwrap();
            Self::take_fault(&mut s)?;
            s.files.get(file_id).ok_or(CloudError::NotFound)?.bytes.clone()
        };
        for (i, chunk) in bytes.chunks(CHUNK).enumerate() {
            sink.write_all(chunk).map_err(|e| CloudError::Permanent(e.to_string()))?;
            if !progress(((i * CHUNK) + chunk.len()) as u64) {
                return Err(CloudError::Cancelled);
            }
        }
        Ok(())
    }

    fn delete(&self, file_id: &str) -> CloudResult<()> {
        let mut s = self.state.lock().unwrap();
        Self::take_fault(&mut s)?;
        s.files.remove(file_id).map(|_| ()).ok_or(CloudError::NotFound)
    }

    fn list_tagged(&self, root_id: &str) -> CloudResult<Vec<RemoteFile>> {
        let mut s = self.state.lock().unwrap();
        Self::take_fault(&mut s)?;
        let root_path = s.folders.iter().find(|(_, id)| *id == root_id).map(|(p, _)| p.clone());
        let Some(root_path) = root_path else { return Ok(Vec::new()) };
        let under: Vec<String> =
            s.folders.iter().filter(|(p, _)| p.starts_with(&root_path)).map(|(_, id)| id.clone()).collect();
        Ok(s.files
            .iter()
            .filter(|(_, f)| under.contains(&f.folder) && f.props.contains_key(props::CLIP_ID))
            .map(|(id, f)| RemoteFile {
                id: id.clone(),
                name: f.name.clone(),
                size: f.bytes.len() as u64,
                hash: None,
                props: f.props.clone(),
            })
            .collect())
    }

    fn space(&self) -> CloudResult<ProviderSpace> {
        let mut s = self.state.lock().unwrap();
        Self::take_fault(&mut s)?;
        Ok(ProviderSpace { free: s.capacity.saturating_sub(Self::used(&s)), total: Some(s.capacity) })
    }

    fn stream(&self, file_id: &str) -> CloudResult<StreamSource> {
        let s = self.state.lock().unwrap();
        s.files.get(file_id).ok_or(CloudError::NotFound)?;
        Ok(StreamSource { url: format!("mem://stream/{file_id}"), headers: Vec::new() })
    }

    fn web_link(&self, file_id: &str) -> Option<String> {
        Some(format!("mem://view/{file_id}"))
    }
}
