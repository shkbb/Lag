//! The cloud rows of the Library tab (docs/spec/cloud.md §7, §11) and what each one changes. The account row
//! is always there (connect / disconnect); the rest appears once a cloud is connected.
use super::*;
use crate::services::cloud::{AccountView, CloudView};
use crate::SettingsRowData;
use lag_cloud::layout::SortMode;
use lag_cloud::settings::{CloudSettings, DeleteMode, UploadWhat, UploadWhen, LIMIT_RUNGS_GB};

const UPLOAD: [UploadWhat; 3] = [UploadWhat::All, UploadWhat::Favorites, UploadWhat::Manual];
const WHEN: [UploadWhen; 2] = [UploadWhen::Immediately, UploadWhen::NoGameRunning];
const SORT: [SortMode; 4] = [SortMode::ByGame, SortMode::ByGameMonth, SortMode::ByMonth, SortMode::Flat];
const DELETE: [DeleteMode; 3] = [DeleteMode::Ask, DeleteMode::ThisPcOnly, DeleteMode::Everywhere];

pub(super) fn rows(view: &CloudView) -> Vec<SettingsRowData> {
    let s = &view.settings;
    let account = view.accounts.first();

    // kind 15: the account. checked = connected, custom = signing in, auto-mode = description is a problem
    let mut acc = row("cloud-account", "Cloud_GoogleDrive", 15);
    acc.checked = account.is_some();
    acc.custom = view.connecting;
    acc.enabled = view.configured || account.is_some();
    match account {
        Some(a) => {
            acc.display = a.settings.display_name.clone().into();
            if let Some(e) = a.error {
                acc.description = e.into();
                acc.auto_mode = true;
            }
        }
        None => {
            acc.description = view.error.unwrap_or("Cloud_Desc").into();
            acc.auto_mode = view.error.is_some();
        }
    }

    let mut space = row("cloud-space", "Cloud_Space", 16);
    space.display = account.map_or_else(String::new, space_text).into();

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

    let (auto_cleanup, max_gb) = account.map_or((false, 50), |a| (a.settings.auto_cleanup, a.settings.max_size_gb));
    let cleanup = super::rows::toggle("cloud-cleanup", "Cloud_AutoCleanup", "Cloud_AutoCleanup_Desc", auto_cleanup);
    let mut limit = choice(
        "cloud-limit",
        "Cloud_Limit",
        0,
        LIMIT_RUNGS_GB.iter().map(|g| (if *g >= 1000 { format!("{} TB", g / 1000) } else { format!("{g} GB") }, 0)).collect(),
        pos(LIMIT_RUNGS_GB, &max_gb),
    );
    limit.enabled = auto_cleanup;

    // what Delete in the library does with a clip that is also in the cloud ("remember my choice" lands here)
    let mut delete = segmented("cloud-delete", "Cloud_DeleteMode", &["Cloud_Delete_Ask", "Delete_ThisPc", "Delete_Everywhere"], pos(&DELETE, &s.delete_mode));
    delete.description = "Cloud_DeleteMode_Desc".into();

    let mut rows = vec![acc, space, upload, when, sort, shots, cleanup, limit, delete];
    for r in rows.iter_mut().skip(1) {
        r.visible = account.is_some();
    }
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

/// A choice on a cloud row.
pub(super) fn choose(cs: &mut CloudSettings, id: &str, index: usize) {
    match id {
        "cloud-upload" => cs.upload_what = UPLOAD.get(index).copied().unwrap_or_default(),
        "cloud-when" => cs.upload_when = WHEN.get(index).copied().unwrap_or_default(),
        "cloud-sort" => cs.sort = SORT.get(index).copied().unwrap_or_default(),
        "cloud-delete" => cs.delete_mode = DELETE.get(index).copied().unwrap_or_default(),
        "cloud-limit" => {
            if let (Some(a), Some(gb)) = (cs.accounts.first_mut(), LIMIT_RUNGS_GB.get(index)) {
                a.max_size_gb = *gb;
            }
        }
        _ => {}
    }
}

/// A switch on a cloud row.
pub(super) fn toggle(cs: &mut CloudSettings, id: &str, value: bool) {
    match id {
        "cloud-shots" => cs.upload_screenshots = value,
        "cloud-cleanup" => {
            if let Some(a) = cs.accounts.first_mut() {
                a.auto_cleanup = value;
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lag_cloud::quota::ProviderSpace;
    use lag_cloud::settings::AccountSettings;
    use lag_cloud::sync::Usage;
    use slint::Model;

    fn connected() -> CloudView {
        let settings = AccountSettings { display_name: "me@example.com".into(), ..Default::default() };
        CloudView {
            settings: CloudSettings { enabled: true, accounts: vec![settings.clone()], ..Default::default() },
            accounts: vec![AccountView {
                settings,
                usage: Some(Usage {
                    lag_bytes: 3 << 30,
                    clips: 4,
                    space: ProviderSpace { free: 4 << 40, total: Some(5 << 40) },
                }),
                error: None,
            }],
            configured: true,
            ..Default::default()
        }
    }

    #[test]
    fn not_connected_shows_only_the_account_row() {
        let shown = rows(&CloudView { configured: true, ..Default::default() });
        assert_eq!(shown[0].id, "cloud-account");
        assert!(!shown[0].checked && shown[0].enabled);
        assert_eq!(shown[0].description, "Cloud_Desc");
        assert!(shown.iter().skip(1).all(|r| !r.visible));
        // a failed sign-in is shown on the row
        let failed = rows(&CloudView { configured: true, error: Some("Cloud_Error_NoDrive"), ..Default::default() });
        assert!(failed[0].auto_mode && failed[0].description == "Cloud_Error_NoDrive");
        // a build without the OAuth client cannot connect
        assert!(!rows(&CloudView::default())[0].enabled);
    }

    #[test]
    fn connected_shows_the_account_its_space_and_the_settings() {
        let shown = rows(&connected());
        assert!(shown[0].checked);
        assert_eq!(shown[0].display, "me@example.com");
        assert!(shown.iter().all(|r| r.visible));
        assert!(shown[1].display.contains("3") && shown[1].display.contains("TB"), "{}", shown[1].display);
        let limit = shown.iter().find(|r| r.id == "cloud-limit").unwrap();
        assert!(!limit.enabled, "auto-cleanup is off by default: its limit is greyed out");
        assert_eq!(limit.options.row_count(), LIMIT_RUNGS_GB.len());
    }

    #[test]
    fn rows_change_the_cloud_settings() {
        let mut cs = connected().settings;
        choose(&mut cs, "cloud-upload", 1);
        choose(&mut cs, "cloud-when", 0);
        choose(&mut cs, "cloud-sort", 3);
        toggle(&mut cs, "cloud-cleanup", true);
        choose(&mut cs, "cloud-limit", 4);
        choose(&mut cs, "cloud-delete", 2);
        assert_eq!(cs.delete_mode, DeleteMode::Everywhere);
        toggle(&mut cs, "cloud-shots", false);
        assert_eq!(
            (cs.upload_what, cs.upload_when, cs.sort, cs.upload_screenshots),
            (UploadWhat::Favorites, UploadWhen::Immediately, SortMode::Flat, false)
        );
        assert_eq!((cs.accounts[0].auto_cleanup, cs.accounts[0].max_size_gb), (true, 250));
    }
}
