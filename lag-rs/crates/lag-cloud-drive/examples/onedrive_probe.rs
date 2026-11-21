//! Diagnoses a OneDrive account with the tokens oauth_live stored (no browser): prints the status and the error
//! body (never tokens) of the calls Lag makes first.
//!   cargo run -p lag-cloud-drive --example onedrive_probe --release -- <user id>
use lag_cloud::{AccountId, ProviderKind};
use lag_cloud_drive::http::{send_collect, Method, Request};
use lag_cloud_drive::tokens::TokenStore;
use lag_cloud_drive::winhttp::WinHttp;
use lag_cloud_drive::{oauth, ClientConfig};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let user = std::env::args().nth(1).ok_or("user id")?;
    let account = AccountId::new(ProviderKind::OneDrive, &user);
    let store = TokenStore::for_user().ok_or("no LOCALAPPDATA")?;
    let mut tokens = store.load(&account).ok_or("no stored tokens for this account")?;
    let client = ClientConfig::onedrive().ok_or("no ONEDRIVE_CLIENT_ID")?;
    let http = WinHttp::new()?;
    oauth::refresh_for(&oauth::MICROSOFT, &http, &client, &mut tokens)?;
    store.save(&account, &tokens)?;
    let t = tokens.access_token.clone();

    let show = |label: &str, request: Request| {
        match send_collect(&http, request) {
            Ok((r, body)) => {
                let text = String::from_utf8_lossy(&body);
                println!("{label}: HTTP {}\n   {}", r.status, text.chars().take(600).collect::<String>());
            }
            Err(e) => println!("{label}: transport error {e}"),
        }
    };
    show("drive", Request::new(Method::Get, "https://graph.microsoft.com/v1.0/me/drive?select=driveType,quota").bearer(&t));
    show("approot", Request::new(Method::Get, "https://graph.microsoft.com/v1.0/me/drive/special/approot?select=id,name,parentReference").bearer(&t));
    show(
        "approot children",
        Request::new(Method::Get, "https://graph.microsoft.com/v1.0/me/drive/special/approot/children?select=name,size").bearer(&t),
    );
    show(
        "make folder",
        Request::new(Method::Post, "https://graph.microsoft.com/v1.0/me/drive/special/approot/children")
            .bearer(&t)
            .json(&serde_json::json!({"name": "Lag test", "folder": {}, "@microsoft.graph.conflictBehavior": "fail"})),
    );
    Ok(())
}
