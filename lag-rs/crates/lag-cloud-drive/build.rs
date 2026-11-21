//! The owner's Google OAuth client (Desktop app) comes from `lag-rs/cloud-secrets.env`, which is never committed
//! (.gitignore). It is compiled in as LAG_GOOGLE_CLIENT_ID / LAG_GOOGLE_CLIENT_SECRET; without the file Lag builds
//! fine and Google Drive reports "not configured". Desktop apps cannot keep a secret; PKCE is what protects the
//! sign-in (Google's own guidance for installed apps).
use std::path::PathBuf;

fn main() {
    let file = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../cloud-secrets.env");
    println!("cargo:rerun-if-changed={}", file.display());
    let Ok(text) = std::fs::read_to_string(&file) else {
        println!("cargo:warning=cloud-secrets.env not found: Google Drive is built without its OAuth client");
        return;
    };
    for line in text.lines().map(str::trim).filter(|l| !l.starts_with('#')) {
        let Some((key, value)) = line.split_once('=') else { continue };
        let value = value.trim();
        match key.trim() {
            "GOOGLE_CLIENT_ID" => println!("cargo:rustc-env=LAG_GOOGLE_CLIENT_ID={value}"),
            "GOOGLE_CLIENT_SECRET" => println!("cargo:rustc-env=LAG_GOOGLE_CLIENT_SECRET={value}"),
            "DROPBOX_APP_KEY" => println!("cargo:rustc-env=LAG_DROPBOX_APP_KEY={value}"),
            "ONEDRIVE_CLIENT_ID" => println!("cargo:rustc-env=LAG_ONEDRIVE_CLIENT_ID={value}"),
            _ => {}
        }
    }
}
