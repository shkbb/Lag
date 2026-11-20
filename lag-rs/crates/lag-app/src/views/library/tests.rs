use super::*;
use super::{clip::*, filter::*, grouping::*, layout::*, scan::*, storage::*, thumbnails::*};
use chrono::NaiveDate;
use lag_core::metadata::ClipMetadataStore;
use lag_core::settings::SettingsData;
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use lag_core::metadata::ClipMetadata;

#[test]
fn squashed_legacy_thumbnails_are_made_again() {
    let dir = std::env::temp_dir().join(format!("lag-thumb-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let write = |name: &str, w: u32, h: u32| {
        let p = dir.join(name);
        image::RgbImage::new(w, h).save(&p).unwrap();
        p
    };
    assert!(is_legacy_thumbnail(&write("old.jpg", 320, 180)));
    assert!(!is_legacy_thumbnail(&write("wide.jpg", 516, 216)));
    assert!(!is_legacy_thumbnail(&write("16x9.jpg", 384, 216)));
    assert!(!is_legacy_thumbnail(&dir.join("missing.jpg")));
    assert!(thumbnail_filter().contains("force_original_aspect_ratio=increase"));
    let _ = std::fs::remove_dir_all(&dir);
}

fn test_temp_dir(name: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-data/library").join(format!(
        "{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

struct FixedFormatter;
impl DateFormatProvider for FixedFormatter {
    fn format_d_mmm(&self, date: NaiveDate) -> String {
        date.format("%-d %b").to_string()
    }
    fn format_d_mmmm(&self, date: NaiveDate) -> String {
        date.format("%-d %B").to_string()
    }
    fn format_dddd(&self, date: NaiveDate) -> String {
        date.format("%A").to_string()
    }
}

#[test]
fn thumbnail_cache_key_matches_csharp_md5_and_mtime() {
    let thumb_root = Path::new(r"C:\Users\Test\AppData\Local\Lag\thumbnails");
    // Verified against .NET 8 MD5.HashData(UTF8(@"C:\Clips\Replay.mp4".ToLowerInvariant()))
    assert_eq!(
        thumbnail_cache_path(thumb_root, r"C:\Clips\Replay.mp4"),
        thumb_root.join("E70506024595A2FBC13F5ECC2858A8C3.jpg")
    );
    assert_eq!(
        thumbnail_cache_path(thumb_root, r"C:\Clips\Replay.mp4"),
        thumbnail_cache_path(thumb_root, r"c:\clips\replay.mp4")
    );

    let dir = test_temp_dir("thumb-key");
    let jpg = dir.join("E70506024595A2FBC13F5ECC2858A8C3.jpg");
    assert!(ThumbnailMemCache::key_for(&jpg).is_none());
    std::fs::write(&jpg, b"fakejpg").unwrap();
    let k1 = ThumbnailMemCache::key_for(&jpg).unwrap();
    assert!(k1.starts_with(&format!("{}|", jpg.to_string_lossy())));
    let ticks_part: i64 = k1.rsplit('|').next().unwrap().parse().unwrap();
    assert!(ticks_part > 630_000_000_000_000_000); // > year 1997 in .NET ticks

    let mut cache = ThumbnailMemCache::default();
    let buf = SharedPixelBuffer::<Rgba8Pixel>::new(2, 2);
    cache.insert(&jpg, buf.clone());
    assert!(cache.get(&jpg).is_some());
    assert_eq!(cache.len(), 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn scan_discovers_all_media_ignores_other_and_subfolders_and_builds_signature() {
    let dir = test_temp_dir("scan");
    let lib = dir.join("lib");
    let meta_dir = dir.join("meta");
    let thumb_dir = dir.join("thumbs");
    let sub = lib.join("subfolder");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::create_dir_all(&sub).unwrap();

    // Create files across all 8 patterns plus ignored files and a subfolder clip
    let names = [
        "clip1.mp4",
        "clip2.MKV",
        "clip3.mov",
        "clip4.avi",
        "anim.gif",
        "shot1.png",
        "shot2.jpg",
        "shot3.JPEG",
        "clip1-edit.mp4",
    ];
    for (i, n) in names.iter().enumerate() {
        std::fs::write(lib.join(n), vec![b'x'; 10 + i]).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(18));
    }
    std::fs::write(lib.join("notes.txt"), b"ignore").unwrap();
    std::fs::write(lib.join("clip1.mp4.json"), b"{}").unwrap();
    std::fs::write(sub.join("nested.mp4"), b"ignore").unwrap();

    let store = ClipMetadataStore::new(&meta_dir);
    store.write(
        &lib.join("clip1.mp4").to_string_lossy(),
        &ClipMetadata {
            game: Some("Valorant".into()),
            exe: Some("VALORANT.exe".into()),
            favorite: true,
            edited: false,
            duration_seconds: Some(12.5),
            cloud_id: None,
        },
    );
    store.write(
        &lib.join("clip3.mov").to_string_lossy(),
        &ClipMetadata { game: None, exe: None, favorite: false, edited: true, duration_seconds: None, cloud_id: None },
    );

    let mem = Mutex::new(ThumbnailMemCache::default());
    let (clips, sig) = scan_library_phase1(&lib, Some(&store), &thumb_dir, &mem, &|| false).unwrap();

    assert_eq!(clips.len(), names.len());
    // Newest-first (clip1-edit.mp4 created last -> first in list)
    assert_eq!(clips[0].display_name(), "clip1-edit");
    assert!(clips[0].is_edited); // fallback "-edit" in filename

    // Check image vs gif classification
    let gif = clips.iter().find(|c| c.display_name() == "anim").unwrap();
    assert!(!gif.is_image);
    assert_ne!(gif.thumbnail_path, gif.file_path);

    let png = clips.iter().find(|c| c.display_name() == "shot1").unwrap();
    assert!(png.is_image);
    assert_eq!(png.thumbnail_path, png.file_path);

    let jpeg = clips.iter().find(|c| c.display_name() == "shot3").unwrap();
    assert!(jpeg.is_image);

    // Check sidecar metadata fields
    let c1 = clips.iter().find(|c| c.display_name() == "clip1").unwrap();
    assert!(c1.is_favorite);
    assert_eq!(c1.game.as_deref(), Some("Valorant"));
    assert_eq!(c1.duration_ticks, 125_000_000);
    assert_eq!(c1.duration_display(), "0:12");

    let c3 = clips.iter().find(|c| c.display_name() == "clip3").unwrap();
    assert!(c3.is_edited);
    assert!(!c3.has_game());

    // Signature has 1 line per clip: path|len|ticks\n
    let lines: Vec<&str> = sig.lines().collect();
    assert_eq!(lines.len(), names.len());
    assert!(lines[0].contains("|18|")); // 10 + 8

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn filter_chips_and_day_grouping_match_csharp() {
    lag_core::i18n::set_language("en");
    let today = NaiveDate::from_ymd_opt(2026, 11, 16).unwrap();
    let yesterday = NaiveDate::from_ymd_opt(2026, 11, 15).unwrap();
    let older = NaiveDate::from_ymd_opt(2026, 11, 10).unwrap();

    let clips = vec![
        ClipEntry {
            file_path: r"C:\Lib\Alpha.mp4".into(),
            game: Some("Valorant".into()),
            is_favorite: true,
            created_date: today.and_hms_opt(14, 5, 0).unwrap(),
            ..ClipEntry::default()
        },
        ClipEntry {
            file_path: r"C:\Lib\Beta.mp4".into(),
            game: Some("valorant".into()), // distinct OrdinalIgnoreCase preserves first casing
            is_favorite: false,
            created_date: today.and_hms_opt(10, 0, 0).unwrap(),
            ..ClipEntry::default()
        },
        ClipEntry {
            file_path: r"C:\Lib\Gamma.png".into(),
            game: None,
            is_image: true,
            is_favorite: true,
            created_date: yesterday.and_hms_opt(22, 30, 0).unwrap(),
            ..ClipEntry::default()
        },
        ClipEntry {
            file_path: r"C:\Lib\Delta.mp4".into(),
            game: Some("Apex Legends".into()),
            is_favorite: false,
            created_date: older.and_hms_opt(18, 0, 0).unwrap(),
            ..ClipEntry::default()
        },
    ];

    let mut filter = Some("NONEXISTENT".to_string());
    let chips = build_game_chips(&clips, &mut filter, "Desktop");
    assert!(filter.is_none()); // stale filter dropped
    assert_eq!(chips.len(), 3);
    assert_eq!(chips[0].label, "Apex Legends");
    assert_eq!(chips[1].label, "Valorant");
    assert_eq!(chips[2].label, "Desktop");
    assert_eq!(chips[2].filter_key, DESKTOP_FILTER_KEY);

    // Filter: favorites + Desktop
    let filtered = filter_clips(&clips, true, Some(DESKTOP_FILTER_KEY), "");
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].0, 2);

    // Filter: search matches DisplayName or Game case-insensitively
    let filtered_q = filter_clips(&clips, false, None, "  VALO ");
    assert_eq!(filtered_q.len(), 2);

    // Grouping: Today, Yesterday, older date descending
    let all: Vec<(usize, &ClipEntry)> = clips.iter().enumerate().collect();
    let groups = group_clips_by_day(&all, today, &FixedFormatter);
    assert_eq!(groups.len(), 3);
    assert_eq!(groups[0].label, "TODAY");
    assert_eq!(groups[0].count_label, "2 clips");
    assert_eq!(groups[0].clip_indices, vec![0, 1]);
    assert_eq!(groups[1].label, "YESTERDAY");
    assert_eq!(groups[1].count_label, "1 clips");
    assert_eq!(groups[2].label, "10 NOVEMBER");
    assert_eq!(groups[2].sub, "Tuesday");
}

#[test]
fn quota_cleanup_deletes_oldest_videos_only_and_virtual_layout_rows() {
    let dir = test_temp_dir("quota");
    let lib = dir.join("lib");
    let meta_dir = dir.join("meta");
    let thumb_dir = dir.join("thumbs");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::create_dir_all(&thumb_dir).unwrap();

    let store = ClipMetadataStore::new(&meta_dir);
    let old_vid = lib.join("old.mp4");
    let shot = lib.join("shot.png");
    let new_vid = lib.join("new.mp4");

    // Total videos: 2 * 600MB? We can't write 1.2GB in a fast unit test, so test quota non-positive
    // and test delete_clip + virtual row layout on synthetic data.
    std::fs::write(&old_vid, vec![1u8; 128]).unwrap();
    std::fs::write(&shot, vec![2u8; 256]).unwrap();
    std::fs::write(&new_vid, vec![3u8; 128]).unwrap();
    // Files written in the same instant share a creation time; make "old" unambiguously older
    // (cleanup orders by CreationTime, like C# LibraryViewModel.cs:300).
    {
        use std::os::windows::fs::FileTimesExt;
        let hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        let f = std::fs::OpenOptions::new().write(true).open(&old_vid).unwrap();
        f.set_times(std::fs::FileTimes::new().set_created(hour_ago)).unwrap();
    }

    let old_thumb = thumbnail_cache_path(&thumb_dir, &old_vid.to_string_lossy());
    std::fs::write(&old_thumb, b"thumb").unwrap();
    store.set_favorite(&old_vid.to_string_lossy(), true);

    // Non-positive quota does nothing
    enforce_storage_quota(&lib, true, 0, Some(&store), &thumb_dir);
    assert!(old_vid.exists());

    // Disabled quota does nothing
    enforce_storage_quota(&lib, false, 1, Some(&store), &thumb_dir);
    assert!(old_vid.exists());

    // Enforce 200-byte limit: total video bytes = 256 (128 + 128), screenshot is 256 (excluded).
    // Should delete oldest video (old.mp4, even though favorited!) along with its metadata & thumb,
    // leaving new.mp4 (128 <= 200) and shot.png intact.
    enforce_storage_quota_bytes(&lib, 200, Some(&store), &thumb_dir);
    assert!(!old_vid.exists());
    assert!(!old_thumb.exists());
    assert!(store.read(&old_vid.to_string_lossy()).is_none());
    assert!(new_vid.exists());
    assert!(shot.exists());

    // Virtual row layout test: 2 groups (5 clips, 2 clips) at 3 columns -> 2 + 1 card rows + 2 headers = 5 rows
    let groups = vec![
        DayGroup {
            label: "TODAY".into(),
            sub: "16 Nov".into(),
            count_label: "5 clips".into(),
            clip_indices: vec![0, 1, 2, 3, 4],
        },
        DayGroup {
            label: "YESTERDAY".into(),
            sub: "15 Nov".into(),
            count_label: "2 clips".into(),
            clip_indices: vec![5, 6],
        },
    ];
    assert_eq!(columns_for_viewport(900.0), 3); // (900 - 76) / 268 = 3.07 -> 3
    let (rows, total_h) = layout_virtual_rows(&groups, 3);
    assert_eq!(rows.len(), 5);
    assert!(total_h > 700.0);
    let (first, last) = visible_row_range(&rows, 0.0, 300.0);
    assert_eq!(first, 0);
    assert!(last <= rows.len());

    // Hotkey parts
    let s = SettingsData::default();
    assert_eq!(hotkey_parts_from_settings(&s), vec!["Alt", "F10"]);

    std::fs::remove_dir_all(&dir).unwrap();
}
