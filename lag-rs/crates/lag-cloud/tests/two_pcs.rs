//! The whole story on the in-memory cloud: PC A uploads, PC B connects the same cloud and sees everything,
//! both change things at the same time, connections drop, the cloud fills up, the index gets lost.
use chrono::{TimeZone, Utc};
use lag_cloud::index::IndexEntry;
use lag_cloud::layout::SortMode;
use lag_cloud::memory::{Fault, MemoryCloud};
use lag_cloud::presence::{join_library, Badge, LocalClip};
use lag_cloud::provider::{CloudError, CloudProvider};
use lag_cloud::queue::UploadSession;
use lag_cloud::settings::WhenFull;
use lag_cloud::sync::{AccountClient, UploadRequest};
use lag_cloud::{AccountId, ClipId, ProviderKind};
use std::collections::HashMap;

fn pc<'a>(cloud: &'a MemoryCloud, device: &str) -> AccountClient<'a> {
    AccountClient { provider: cloud, root: "Lag".into(), sort: SortMode::ByGame, device: device.into() }
}

fn clip(name: &str, game: Option<&str>, size: u64, minute: u32) -> IndexEntry {
    IndexEntry {
        file_name: name.into(),
        game: game.map(Into::into),
        size,
        duration_seconds: Some(120.0),
        created: Utc.with_ymd_and_hms(2026, 11, 20, 12, minute, 0).unwrap(),
        ..Default::default()
    }
}

fn upload(client: &AccountClient, id: ClipId, entry: IndexEntry, bytes: &[u8]) -> Result<IndexEntry, CloudError> {
    let mut source = bytes;
    let req = UploadRequest { id, entry, source: &mut source, thumbnail_jpeg: Some(b"jpg".to_vec()) };
    client.upload_clip(req, None, &mut |_| {}, &mut |_| true)
}

#[test]
fn a_second_pc_sees_the_library_and_changes_flow_both_ways() {
    let cloud = MemoryCloud::new(1_000);
    let (a, b) = (pc(&cloud, "PC-A"), pc(&cloud, "PC-B"));
    let (cs, desk) = (ClipId::new_random(), ClipId::new_random());
    upload(&a, cs, clip("cs.mp4", Some("Counter-Strike 2"), 12, 1), b"counterstrik").unwrap();
    upload(&a, desk, clip("desk.mp4", None, 8, 2), b"desktop!").unwrap();

    // the tree people see in their cloud
    assert_eq!(cloud.file_names(&["Lag", "Counter-Strike 2"]), ["cs.mp4"]);
    assert_eq!(cloud.file_names(&["Lag", "Desktop"]), ["desk.mp4"]);
    assert_eq!(cloud.file_names(&["Lag", ".lag", "thumbs"]).len(), 2);

    // PC B connects: the whole library, cloud-only, playable from the cloud, with thumbnails
    let (index_b, _) = b.load_index().unwrap();
    let drive = AccountId::new(ProviderKind::GoogleDrive, "me");
    let items = join_library(&[], &[(drive.clone(), &index_b)], &HashMap::new());
    assert_eq!(items.len(), 2);
    assert!(items.iter().all(|i| i.presence.badge() == Badge::CloudOnly));
    let cs_entry = &index_b.entries[&cs];
    assert_eq!(cs_entry.uploaded_by.as_deref(), Some("PC-A"));
    assert!(cs_entry.thumbnail_id.is_some());
    assert!(b.stream(cs_entry).unwrap().url.contains(cs_entry.remote_id.as_deref().unwrap()));
    // "Save to this PC": the download is the same bytes, and B's library then shows it once, as synced
    let mut saved = Vec::new();
    cloud.download(cs_entry.remote_id.as_deref().unwrap(), &mut saved, &mut |_| true).unwrap();
    assert_eq!(saved, b"counterstrik");
    let here = [LocalClip { id: Some(cs), path: "D:/cs.mp4".into(), file_name: "cs.mp4".into(), size: 12, duration_seconds: Some(120.0) }];
    let items = join_library(&here, &[(drive, &index_b)], &HashMap::new());
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].presence.badge(), Badge::Synced);

    // B stars a clip while A uploads a new one: both changes survive
    let new = ClipId::new_random();
    b.sync_meta(cs, &|e| e.favorite = true).unwrap();
    upload(&a, new, clip("new.mp4", Some("Counter-Strike 2"), 4, 3), b"neww").unwrap();
    let (index_a, _) = a.load_index().unwrap();
    assert!(index_a.entries[&cs].favorite);
    assert_eq!(index_a.live().count(), 3);

    // B deletes a clip everywhere: files gone, A sees it gone
    let before = cloud.file_count();
    b.delete_clip(desk).unwrap();
    assert_eq!(cloud.file_count(), before - 2, "video + thumbnail");
    let (index_a, _) = a.load_index().unwrap();
    assert!(!index_a.entries[&desk].is_live());
    assert_eq!(index_a.live().count(), 2);
}

#[test]
fn another_pc_writing_the_index_at_the_same_moment_loses_nothing() {
    let cloud = MemoryCloud::new(1_000);
    let (a, b) = (pc(&cloud, "A"), pc(&cloud, "B"));
    let first = ClipId::new_random();
    upload(&a, first, clip("1.mp4", None, 4, 1), b"1111").unwrap();
    // right before A writes, B's version (with B's own clip) lands in the cloud
    let theirs_id = ClipId::new_random();
    let mut theirs = b.load_index().unwrap().0;
    theirs.upsert(theirs_id, clip("b.mp4", None, 1, 9), Utc::now());
    cloud.race_next_write(theirs.to_json().into_bytes());
    let mine = ClipId::new_random();
    upload(&a, mine, clip("2.mp4", None, 4, 2), b"2222").unwrap();
    let (index, _) = a.load_index().unwrap();
    for id in [first, theirs_id, mine] {
        assert!(index.entries[&id].is_live(), "{id} kept");
    }
}

#[test]
fn a_dropped_connection_resumes_where_it_stopped() {
    let cloud = MemoryCloud::new(1_000);
    let a = pc(&cloud, "A");
    let id = ClipId::new_random();
    let data = b"0123456789abcdef";
    cloud.inject(Fault::DropUploadAfter(8));
    let mut session: Option<UploadSession> = None;
    let mut source: &[u8] = data;
    let req = UploadRequest { id, entry: clip("r.mp4", None, 16, 1), source: &mut source, thumbnail_jpeg: None };
    let err = a.upload_clip(req, None, &mut |s| session = Some(s), &mut |_| true).unwrap_err();
    assert!(err.is_retryable());
    let session = session.unwrap();
    assert_eq!(session.offset, 8, "the queue persisted how far it got");
    // retry: the caller positions the file at the offset and passes the session
    let mut rest: &[u8] = &data[session.offset as usize..];
    let req = UploadRequest { id, entry: clip("r.mp4", None, 16, 1), source: &mut rest, thumbnail_jpeg: None };
    let stored = a.upload_clip(req, Some(&session), &mut |_| {}, &mut |_| true).unwrap();
    assert_eq!(cloud.file_bytes(stored.remote_id.as_deref().unwrap()).unwrap(), data);
    assert_eq!(cloud.file_count(), 1, "one file, not a half and a whole");
}

#[test]
fn a_full_cloud_cleans_up_its_oldest_clips_but_never_favorites() {
    let cloud = MemoryCloud::new(40);
    let a = pc(&cloud, "A");
    let ids: Vec<ClipId> = (0..3).map(|_| ClipId::new_random()).collect();
    for (i, id) in ids.iter().enumerate() {
        upload(&a, *id, clip(&format!("{i}.mp4"), None, 10, i as u32), b"0123456789").unwrap();
    }
    a.sync_meta(ids[0], &|e| e.favorite = true).unwrap();
    // 30 bytes of clips + 3 thumbnails of 3 bytes = 39 of 40: a 10-byte clip does not fit
    let big = ClipId::new_random();
    assert_eq!(upload(&a, big, clip("big.mp4", None, 10, 9), b"0123456789").unwrap_err(), CloudError::Full);
    // "stop uploads": nothing removed
    assert!(a.make_room(0, WhenFull::StopUploads, 10).unwrap().full);
    // auto-cleanup: the oldest NON-favorite goes, then it fits
    let plan = a.make_room(0, WhenFull::RemoveOldest, 10).unwrap();
    assert_eq!((plan.delete, plan.full), (vec![ids[1]], false));
    upload(&a, big, clip("big.mp4", None, 10, 9), b"0123456789").unwrap();
    let (index, _) = a.load_index().unwrap();
    assert!(index.entries[&ids[0]].is_live(), "the favorite stays");
    assert!(!index.entries[&ids[1]].is_live());
}

#[test]
fn a_lost_index_is_rebuilt_from_the_files_even_after_the_user_moved_them() {
    let cloud = MemoryCloud::new(1_000);
    let a = pc(&cloud, "A");
    let id = ClipId::new_random();
    let stored = upload(&a, id, clip("cs.mp4", Some("Counter-Strike 2"), 4, 1), b"cs!!").unwrap();
    cloud.user_moves(stored.remote_id.as_deref().unwrap(), &["Lag", "best of"]);
    // the index file gets corrupted
    let path = lag_cloud::layout::index_path("Lag");
    let (_, rev) = cloud.read_small(&path).unwrap().unwrap();
    cloud.write_small(&path, b"{ not json", Some(&rev)).unwrap();
    let (index, _) = a.load_index().unwrap();
    let e = &index.entries[&id];
    assert_eq!((e.game.as_deref(), e.size, e.duration_seconds), (Some("Counter-Strike 2"), 4, Some(120.0)));
    assert_eq!(e.remote_id, stored.remote_id);
    assert!(e.thumbnail_id.is_some());
    // and the next change writes a good index again
    a.sync_meta(id, &|e| e.favorite = true).unwrap();
    assert!(a.load_index().unwrap().0.entries[&id].favorite);
}

#[test]
fn an_expired_sign_in_reaches_the_caller() {
    let cloud = MemoryCloud::new(1_000);
    let a = pc(&cloud, "A");
    cloud.inject(Fault::Next(CloudError::SignIn));
    assert_eq!(a.load_index().unwrap_err(), CloudError::SignIn);
    assert!(a.load_index().is_ok(), "once signed in again it works");
}
