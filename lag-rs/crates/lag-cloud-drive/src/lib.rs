//! Google Drive for Lag's cloud storage (docs/spec/cloud.md §9):
//! * `oauth`   sign-in with PKCE and a loopback redirect, token refresh and revoke;
//! * `tokens`  the encrypted (DPAPI) token store, one file per account;
//! * `drive`   the Drive REST v3 provider behind `lag_cloud::provider::CloudProvider`;
//! * `http`    the small HTTP trait the provider talks through (scripted in tests);
//! * `winhttp` the real transport; `win` SHA-256, random bytes, DPAPI, opening the browser.
pub mod drive;
pub mod http;
pub mod oauth;
pub mod tokens;
pub mod win;
pub mod winhttp;

pub use drive::GoogleDrive;
pub use oauth::{ClientConfig, SignInError, Tokens};
