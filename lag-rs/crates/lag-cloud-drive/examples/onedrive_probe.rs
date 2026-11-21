//! Diagnoses a OneDrive account with the tokens oauth_live stored (no browser): prints the status and the error
//! body (never tokens) of the calls Lag makes, then removes what it made.
//!   cargo run -p lag-cloud-drive --example onedrive_probe --release -- <user id>
use lag_cloud::{AccountId, ProviderKind};
use lag_cloud_drive::http::{send_collect, Method, Request};
use lag_cloud_drive::tokens::TokenStore;
use lag_cloud_drive::winhttp::WinHttp;
use lag_cloud_drive::{oauth, ClientConfig};
use serde_json::{json, Value};

const G: &str = "https://graph.microsoft.com/v1.0";

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

    let show = |label: &str, request: Request| -> Value {
        match send_collect(&http, request) {
            Ok((r, body)) => {
                let text = String::from_utf8_lossy(&body);
                let short: String = text.chars().take(300).collect();
                println!("{label}: HTTP {}\n   {short}", r.status);
                serde_json::from_slice(&body).unwrap_or(Value::Null)
            }
            Err(e) => {
                println!("{label}: transport error {e}");
                Value::Null
            }
        }
    };
    let root = show("approot", Request::new(Method::Get, format!("{G}/me/drive/special/approot?select=id")).bearer(&t));
    let root_id = root["id"].as_str().unwrap_or_default().to_owned();
    show("get by path (absent)", Request::new(Method::Get, format!("{G}/me/drive/special/approot:/Lag%20probe:?select=id")).bearer(&t));
    let folder = json!({"name": "Lag probe", "folder": {}, "@microsoft.graph.conflictBehavior": "fail"});
    let made = show(
        "make folder under items/<approot id>",
        Request::new(Method::Post, format!("{G}/me/drive/items/{root_id}/children")).bearer(&t).json(&folder),
    );
    let made_id = made["id"].as_str().unwrap_or_default().to_owned();
    show(
        "make folder, no conflictBehavior, approot/children",
        Request::new(Method::Post, format!("{G}/me/drive/special/approot/children"))
            .bearer(&t)
            .json(&json!({"name": "Lag probe 2", "folder": {}})),
    );
    show("get by path (now there)", Request::new(Method::Get, format!("{G}/me/drive/special/approot:/Lag%20probe:?select=id")).bearer(&t));
    show(
        "small PUT by path",
        Request::new(Method::Put, format!("{G}/me/drive/special/approot:/Lag%20probe/a.txt:/content"))
            .bearer(&t)
            .bytes("application/octet-stream", b"hello".to_vec()),
    );
    show(
        "upload session by path",
        Request::new(Method::Post, format!("{G}/me/drive/special/approot:/Lag%20probe/b.mp4:/createUploadSession"))
            .bearer(&t)
            .json(&json!({"item": {"@microsoft.graph.conflictBehavior": "replace"}})),
    );
    show("children of the new folder", Request::new(Method::Get, format!("{G}/me/drive/items/{made_id}/children?select=name,size")).bearer(&t));
    // cleanup
    for name in ["Lag%20probe", "Lag%20probe%202"] {
        show(&format!("delete {name}"), Request::new(Method::Delete, format!("{G}/me/drive/special/approot:/{name}")).bearer(&t));
    }
    Ok(())
}
