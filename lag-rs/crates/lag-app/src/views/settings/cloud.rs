//! The cloud rows of the Library tab (docs/spec/cloud.md §6, §7, §11) and what each one changes. Every
//! connected storage has its own rows (name, space, role when there are several, its auto-cleanup and limit);
//! "Add storage" offers Google Drive, a network folder / NAS and WebDAV; the rules for all of them (what and
//! when to upload, folders, screenshots, delete) follow once any storage is connected.
//! Row ids of a storage carry its index: `cloud-role:1`, `cloud-limit:0`...
use super::*;
use crate::services::cloud::{AccountView, CloudView};
use crate::SettingsRowData;
use lag_cloud::layout::SortMode;
use lag_cloud::settings::{CloudSettings, DeleteMode, Role, UploadWhat, UploadWhen, LIMIT_RUNGS_GB};
use lag_cloud::ProviderKind;

const UPLOAD: [UploadWhat; 3] = [UploadWhat::All, UploadWhat::Favorites, UploadWhat::Manual];
const WHEN: [UploadWhen; 2] = [UploadWhen::Immediately, UploadWhen::NoGameRunning];
const SORT: [SortMode; 4] = [SortMode::ByGame, SortMode::ByGameMonth, SortMode::ByMonth, SortMode::Flat];
const DELETE: [DeleteMode; 3] = [DeleteMode::Ask, DeleteMode::ThisPcOnly, DeleteMode::Everywhere];
const ROLE: [Role; 3] = [Role::Primary, Role::Mirror, Role::Paused];

fn provider_label(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::GoogleDrive => "Cloud_GoogleDrive",
        ProviderKind::Dropbox => "Cloud_Dropbox",
        ProviderKind::OneDrive => "Cloud_OneDrive",
        ProviderKind::Folder => "Cloud_Folder",
        ProviderKind::WebDav => "Cloud_WebDav",
    }
}

fn limit_options() -> Vec<(String, i32)> {
    LIMIT_RUNGS_GB.iter().map(|g| (if *g >= 1000 { format!("{} TB", g / 1000) } else { format!("{g} GB") }, 0)).collect()
}

/// The rows of one connected storage.
fn account_rows(i: usize, a: &AccountView, several: bool) -> Vec<SettingsRowData> {
    let mut acc = row(&format!("cloud-account:{i}"), provider_label(a.settings.provider), 15);
    acc.toggle_id = i.to_string().into();
    acc.display = a.settings.display_name.clone().into();
    if let Some(e) = a.error {
        acc.description = e.into();
        acc.auto_mode = true;
    }
    let mut space = row(&format!("cloud-space:{i}"), "Cloud_Space", 16);
    space.display = space_text(a).into();
    let mut rows = vec![acc, space];
    if several {
        let mut role = choice(
            &format!("cloud-role:{i}"),
            "Cloud_Role",
            1,
            simple_options(&["Cloud_Role_Primary", "Cloud_Role_Mirror", "Cloud_Role_Paused"]),
            pos(&ROLE, &a.settings.role),
        );
        role.description = "Cloud_Role_Desc".into();
        rows.push(role);
    }
    let cleanup = super::rows::toggle(&format!("cloud-cleanup:{i}"), "Cloud_AutoCleanup", "Cloud_AutoCleanup_Desc", a.settings.auto_cleanup);
    let mut limit = choice(&format!("cloud-limit:{i}"), "Cloud_Limit", 0, limit_options(), pos(LIMIT_RUNGS_GB, &a.settings.max_size_gb));
    limit.enabled = a.settings.auto_cleanup;
    rows.extend([cleanup, limit]);
    rows
}

pub(super) fn rows(view: &CloudView) -> Vec<SettingsRowData> {
    let s = &view.settings;
    let several = view.accounts.len() > 1;
    let mut rows: Vec<SettingsRowData> =
        view.accounts.iter().enumerate().flat_map(|(i, a)| account_rows(i, a, several)).collect();

    // kind 17: add a storage. custom = a sign-in / check is running; enabled = Google sign-in available
    let mut add = row("cloud-add", if view.accounts.is_empty() { "Cloud_Connect_Title" } else { "Cloud_Add" }, 17);
    add.custom = view.connecting;
    // which sign-ins this build has: enabled = Google Drive, checked = Dropbox, with-toggle = OneDrive
    add.enabled = view.configured;
    add.checked = view.dropbox_ready;
    add.with_toggle = view.onedrive_ready;
    // already connected (bits: 1 Google Drive, 2 Dropbox, 4 OneDrive): no second sign-in for the same storage
    add.selected = view.accounts.iter().fold(0, |bits, a| {
        bits | match a.settings.provider {
            ProviderKind::GoogleDrive => 1,
            ProviderKind::Dropbox => 2,
            ProviderKind::OneDrive => 4,
            _ => 0,
        }
    });
    add.description = view.error.unwrap_or(if view.accounts.is_empty() { "Cloud_Desc" } else { "" }).into();
    add.auto_mode = view.error.is_some();
    rows.push(add);

    if view.accounts.is_empty() {
        return rows;
    }
    let segmented = |id: &str, label: &str, keys: &[&str], selected: usize| choice(id, label, 1, simple_options(keys), selected);
    let upload = segmented(
        "cloud-upload",
        "Cloud_Upload",
        &["Cloud_Upload_All", "Cloud_Upload_Favorites", "Cloud_Upload_Manual"],
        pos(&UPLOAD, &s.upload_what),
    );
    let when_now = if s.upload_when == UploadWhen::Immediately { 0 } else { 1 };
    let mut when = segmented("cloud-when", "Cloud_When", &["Cloud_When_Now", "Cloud_When_NoGame"], when_now);
    when.enabled = s.upload_what != UploadWhat::Manual;
    let mut sort = choice(
        "cloud-sort",
        "Cloud_Sort",
        0,
        simple_options(&["Cloud_Sort_Game", "Cloud_Sort_GameMonth", "Cloud_Sort_Month", "Cloud_Sort_Flat"]),
        pos(&SORT, &s.sort),
    );
    sort.description = "Cloud_Sort_Desc".into();
    let shots = super::rows::toggle("cloud-shots", "Cloud_Screenshots", "", s.upload_screenshots);
    // what Delete in the library does with a clip that is also in the cloud ("remember my choice" lands here)
    let mut delete = segmented(
        "cloud-delete",
        "Cloud_DeleteMode",
        &["Cloud_Delete_Ask", "Delete_ThisPc", "Delete_Everywhere"],
        pos(&DELETE, &s.delete_mode),
    );
    delete.description = "Cloud_DeleteMode_Desc".into();
    rows.extend([upload, when, sort, shots, delete]);
    rows
}

/// "Lag: 1.2 GB · free 4.8 TB of 5 TB" (or "checking..." until the cloud answers).
fn space_text(a: &AccountView) -> String {
    let Some(u) = a.usage else {
        return tr(if a.error.is_some() { "Cloud_Space_Unknown" } else { "Cloud_Space_Loading" });
    };
    let lag = size(u.lag_bytes);
    // two placeholders per template at most (i18n tests): "free {1}" gets "4.8 TB of 5 TB" built first
    match u.space.total {
        _ if u.space.is_unknown() => i18n::format("Cloud_Space_Hidden", &[&lag]),
        Some(total) => i18n::format("Cloud_Space_FreeOf", &[&size(u.space.free), &size(total)])
            .and_then(|free| i18n::format("Cloud_Space_Value", &[&lag, &free])),
        None => i18n::format("Cloud_Space_Unlimited", &[&lag]),
    }
    .unwrap_or(lag)
}

fn size(bytes: u64) -> String {
    const GIB: f64 = 1_073_741_824.0;
    let gb = bytes as f64 / GIB;
    if gb >= 1000.0 {
        format!("{} TB", lag_core::hardware::fixed_one(gb / 1024.0))
    } else if gb >= 1.0 {
        format!("{} GB", lag_core::hardware::fixed_one(gb))
    } else {
        format!("{} MB", (bytes as f64 / 1_048_576.0).round())
    }
}

/// `cloud-role:2` -> ("cloud-role", Some(2)).
fn split(id: &str) -> (&str, Option<usize>) {
    match id.split_once(':') {
        Some((name, i)) => (name, i.parse().ok()),
        None => (id, None),
    }
}

/// A choice on a cloud row.
pub(super) fn choose(cs: &mut CloudSettings, id: &str, index: usize) {
    let (name, account) = split(id);
    let account = account.and_then(|i| cs.accounts.get_mut(i));
    match (name, account) {
        ("cloud-upload", _) => cs.upload_what = UPLOAD.get(index).copied().unwrap_or_default(),
        ("cloud-when", _) => cs.upload_when = WHEN.get(index).copied().unwrap_or_default(),
        ("cloud-sort", _) => cs.sort = SORT.get(index).copied().unwrap_or_default(),
        ("cloud-delete", _) => cs.delete_mode = DELETE.get(index).copied().unwrap_or_default(),
        ("cloud-limit", Some(a)) => {
            if let Some(gb) = LIMIT_RUNGS_GB.get(index) {
                a.max_size_gb = *gb;
            }
        }
        ("cloud-role", Some(a)) => {
            let role = ROLE.get(index).copied().unwrap_or_default();
            let id = a.id.clone();
            // one primary: choosing a new one makes the old one a mirror
            if role == Role::Primary {
                for other in cs.accounts.iter_mut().filter(|o| o.id != id && o.role == Role::Primary) {
                    other.role = Role::Mirror;
                }
            }
            if let Some(a) = cs.accounts.iter_mut().find(|o| o.id == id) {
                a.role = role;
            }
        }
        _ => {}
    }
}

/// A switch on a cloud row.
pub(super) fn toggle(cs: &mut CloudSettings, id: &str, value: bool) {
    let (name, account) = split(id);
    match (name, account.and_then(|i| cs.accounts.get_mut(i))) {
        ("cloud-shots", _) => cs.upload_screenshots = value,
        ("cloud-cleanup", Some(a)) => a.auto_cleanup = value,
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lag_cloud::quota::ProviderSpace;
    use lag_cloud::settings::AccountSettings;
    use lag_cloud::sync::Usage;
    use lag_cloud::AccountId;
    use slint::Model;

    fn account(kind: ProviderKind, name: &str, role: Role) -> AccountSettings {
        AccountSettings { id: AccountId::new(kind, name), provider: kind, display_name: name.into(), role, ..Default::default() }
    }

    fn view(accounts: Vec<AccountSettings>) -> CloudView {
        CloudView {
            settings: CloudSettings { enabled: !accounts.is_empty(), accounts: accounts.clone(), ..Default::default() },
            accounts: accounts
                .into_iter()
                .map(|settings| AccountView {
                    settings,
                    usage: Some(Usage { lag_bytes: 3 << 30, clips: 4, space: ProviderSpace { free: 4 << 40, total: Some(5 << 40) } }),
                    error: None,
                })
                .collect(),
            configured: true,
            ..Default::default()
        }
    }

    #[test]
    fn nothing_connected_offers_every_kind_of_storage() {
        let shown = rows(&view(vec![]));
        assert_eq!(shown.len(), 1);
        assert_eq!((shown[0].kind, shown[0].description.as_str()), (17, "Cloud_Desc"));
        let failed = rows(&CloudView { configured: true, error: Some("Cloud_Error_WebDavLogin"), ..Default::default() });
        assert!(failed[0].auto_mode && failed[0].description == "Cloud_Error_WebDavLogin");
        assert_eq!(shown[0].selected, 0, "nothing marked connected");
    }

    #[test]
    fn connected_storages_are_marked_on_the_add_row() {
        let shown = rows(&view(vec![
            account(ProviderKind::GoogleDrive, "g", Role::Primary),
            account(ProviderKind::Dropbox, "d", Role::Mirror),
            account(ProviderKind::Folder, "f", Role::Mirror),
        ]));
        let add = shown.iter().find(|r| r.kind == 17).unwrap();
        assert_eq!(add.selected, 1 | 2, "Google Drive + Dropbox; a folder can be added again");
    }

    #[test]
    fn every_storage_has_its_rows_and_roles_appear_with_several() {
        let one = rows(&view(vec![account(ProviderKind::GoogleDrive, "me@example.com", Role::Primary)]));
        assert_eq!(one[0].display, "me@example.com");
        assert!(one[1].display.contains("TB"), "{}", one[1].display);
        assert!(!one.iter().any(|r| r.id.starts_with("cloud-role")), "one storage: no roles");
        let limit = one.iter().find(|r| r.id == "cloud-limit:0").unwrap();
        assert!(!limit.enabled, "auto-cleanup off by default: its limit is greyed out");
        assert_eq!(limit.options.row_count(), LIMIT_RUNGS_GB.len());
        assert!(one.iter().any(|r| r.id == "cloud-add") && one.iter().any(|r| r.id == "cloud-upload"));

        let two = rows(&view(vec![
            account(ProviderKind::GoogleDrive, "me@example.com", Role::Primary),
            account(ProviderKind::Folder, r"\\nas\clips", Role::Mirror),
        ]));
        assert_eq!(two.iter().filter(|r| r.kind == 15).count(), 2);
        assert_eq!(two.iter().find(|r| r.id == "cloud-account:1").unwrap().label, "Cloud_Folder");
        assert_eq!(two.iter().find(|r| r.id == "cloud-role:1").unwrap().selected, 1);
    }

    #[test]
    fn rows_change_the_right_storage() {
        let mut cs = view(vec![
            account(ProviderKind::GoogleDrive, "a", Role::Primary),
            account(ProviderKind::WebDav, "b", Role::Mirror),
        ])
        .settings;
        choose(&mut cs, "cloud-upload", 1);
        choose(&mut cs, "cloud-when", 0);
        choose(&mut cs, "cloud-sort", 3);
        choose(&mut cs, "cloud-delete", 2);
        toggle(&mut cs, "cloud-cleanup:1", true);
        choose(&mut cs, "cloud-limit:1", 4);
        toggle(&mut cs, "cloud-shots", false);
        assert_eq!(
            (cs.upload_what, cs.upload_when, cs.sort, cs.upload_screenshots, cs.delete_mode),
            (UploadWhat::Favorites, UploadWhen::Immediately, SortMode::Flat, false, DeleteMode::Everywhere)
        );
        assert_eq!((cs.accounts[0].auto_cleanup, cs.accounts[1].auto_cleanup, cs.accounts[1].max_size_gb), (false, true, 250));
        // making the mirror the primary demotes the old primary
        choose(&mut cs, "cloud-role:1", 0);
        assert_eq!((cs.accounts[0].role, cs.accounts[1].role), (Role::Mirror, Role::Primary));
    }
}
