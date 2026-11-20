//! Live check against the owner's real Google Drive (run only with his OK: it opens the browser to sign in).
//! Works in its own root folder "Lag test" and removes everything it uploaded at the end.
//!   cargo run -p lag-cloud-drive --example drive_live --release
use chrono::Utc;
use lag_cloud::index::IndexEntry;
use lag_cloud::layout::SortMode;
use lag_cloud::provider::CloudProvider;
use lag_cloud::sync::{AccountClient, UploadRequest};
use lag_cloud::{AccountId, ClipId, ProviderKind};
use lag_cloud_drive::tokens::TokenStore;
use lag_cloud_drive::winhttp::WinHttp;
use lag_cloud_drive::{oauth, win, ClientConfig, GoogleDrive};
use std::io::Cursor;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_048_576.0)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = ClientConfig::built_in().ok_or("no OAuth client: lag-rs/cloud-secrets.env missing at build time")?;
    let http = Arc::new(WinHttp::new()?);

    println!("1. sign-in: the browser opens Google's page (5 min to finish)...");
    let tokens = oauth::sign_in(http.as_ref(), &client, &win::open_browser, Duration::from_secs(300), &AtomicBool::new(false))?;
    let account = AccountId::new(ProviderKind::GoogleDrive, &tokens.sub);
    println!("   signed in as {} ({account})", tokens.email);
    let store = TokenStore::for_user().ok_or("no LOCALAPPDATA")?;
    store.save(&account, &tokens)?;
    let reloaded = store.load(&account).ok_or("stored tokens could not be read back")?;
    assert_eq!(reloaded.refresh_token, tokens.refresh_token);
    println!("   tokens stored encrypted and read back");

    let saver = TokenStore::for_user().ok_or("no LOCALAPPDATA")?;
    let account_for_save = account.clone();
    let drive = GoogleDrive::new(http.clone(), client, tokens, move |t| {
        let _ = saver.save(&account_for_save, t);
    });
    let device = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "PC".into());
    let pc = AccountClient { provider: &drive, root: "Lag test".into(), sort: SortMode::ByGame, device };

    let usage = pc.usage()?;
    println!(
        "2. space: Lag test uses {} in {} clips; Drive free {} of {}",
        mb(usage.lag_bytes),
        usage.clips,
        mb(usage.space.free.min(u64::MAX / 2)),
        usage.space.total.map_or("unlimited".into(), mb)
    );

    // 20 MB of non-trivial data: three resumable chunks
    let size = 20 * 1024 * 1024u64;
    let data: Vec<u8> = (0..size).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
    let id = ClipId::new_random();
    let entry = IndexEntry {
        file_name: format!("lag-live-test-{}.mp4", Utc::now().format("%Y%m%d-%H%M%S")),
        game: Some("Live check".into()),
        created: Utc::now(),
        duration_seconds: Some(12.5),
        size,
        ..Default::default()
    };
    println!("3. upload {} as {}...", mb(size), entry.file_name);
    let started = Instant::now();
    let mut source = Cursor::new(&data);
    let mut last = 0u64;
    let stored = pc.upload_clip(
        UploadRequest { id, entry, source: &mut source, thumbnail_jpeg: Some(b"\xFF\xD8\xFF\xD9".to_vec()) },
        None,
        &mut |_| {},
        &mut |done| {
            if done - last >= 8 * 1024 * 1024 || done == size {
                println!("   {} / {}", mb(done), mb(size));
                last = done;
            }
            true
        },
    )?;
    println!(
        "   done in {:.1} s ({:.1} MB/s), folder {:?}",
        started.elapsed().as_secs_f64(),
        size as f64 / 1_048_576.0 / started.elapsed().as_secs_f64(),
        stored.folder
    );
    let remote = stored.remote_id.clone().ok_or("no remote id")?;

    println!("4. index + download check...");
    let (index, _) = pc.load_index()?;
    let e = index.entries.get(&id).ok_or("clip not in the cloud index")?;
    assert_eq!((e.size, e.game.as_deref()), (size, Some("Live check")));
    assert!(e.thumbnail_id.is_some(), "thumbnail uploaded");
    let mut back = Vec::new();
    drive.download(&remote, &mut back, &mut |_| true)?;
    assert!(back == data, "downloaded bytes differ");
    println!("   index has it, {} came back byte for byte", mb(back.len() as u64));

    let stream = pc.stream(e)?;
    println!("5. stream URL ready ({}...)", &stream.url[..stream.url.len().min(60)]);
    let rescanned = pc.rescan()?;
    assert!(rescanned.entries.contains_key(&id), "rescan finds the clip by its properties");
    println!("   rescan finds it without the index");

    println!("6. cleanup...");
    pc.delete_clip(id)?;
    let (index, _) = pc.load_index()?;
    assert!(!index.entries[&id].is_live());
    println!("   clip + thumbnail deleted, tombstone written");
    println!("ALL OK. (The empty \"Lag test\" folder stays in Drive; delete it by hand if you like.)");
    Ok(())
}
