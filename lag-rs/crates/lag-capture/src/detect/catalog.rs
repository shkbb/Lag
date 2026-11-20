//! What we know by name: titles of popular games, window classes of game engines, and processes that are never
//! a game (the Windows shell, browsers, chat and media apps, launchers). Most games are recognised without
//! these lists (library.rs: the games Windows itself knows, launcher install folders); KNOWN mainly gives
//! nicer names and keeps detection working where those sources are empty.

pub(super) const KNOWN: &[(&str, &str)] = &[
    ("cs2", "Counter-Strike 2"),
    ("csgo", "CS:GO"),
    ("dota2", "Dota 2"),
    ("deadlock", "Deadlock"),
    ("valorant-win64-shipping", "VALORANT"),
    ("valorant", "VALORANT"),
    ("r5apex", "Apex Legends"),
    ("r5apexdx12", "Apex Legends"),
    ("fortniteclient-win64-shipping", "Fortnite"),
    ("gta5", "GTA V"),
    ("gtav", "GTA V"),
    ("gta-v", "GTA V"),
    ("rocketleague", "Rocket League"),
    ("league of legends", "League of Legends"),
    ("leagueoflegends", "League of Legends"),
    ("overwatch", "Overwatch 2"),
    ("tslgame", "PUBG: BATTLEGROUNDS"),
    ("rainbowsix", "Rainbow Six Siege"),
    ("rainbowsix_vulkan", "Rainbow Six Siege"),
    ("destiny2", "Destiny 2"),
    ("bf2042", "Battlefield 2042"),
    ("bf1", "Battlefield 1"),
    ("bfv", "Battlefield V"),
    ("eldenring", "Elden Ring"),
    ("marvel-win64-shipping", "Marvel Rivals"),
    ("discovery", "THE FINALS"),
    ("helldivers2", "Helldivers 2"),
    ("palworld-win64-shipping", "Palworld"),
    ("palworld", "Palworld"),
    ("rust", "Rust"),
    ("dayz", "DayZ"),
    ("warthunder", "War Thunder"),
    ("aces", "War Thunder"),
    ("modernwarfare", "Call of Duty"),
    ("cod", "Call of Duty"),
    ("minecraft.windows", "Minecraft"),
    ("hl2", "Half-Life 2"),
    ("portal2", "Portal 2"),
];
pub(super) const ENGINES: &[&str] =
    &["UnrealWindow", "UnityWndClass", "SDL_app", "riotWindowClass", "CryENGINE", "Valve001", "Sledgehammer Oasis"];
pub(super) const SHELL: &[&str] = &[
    "explorer.exe",
    "dwm.exe",
    "applicationframehost.exe",
    "searchhost.exe",
    "shellexperiencehost.exe",
    "startmenuexperiencehost.exe",
    "textinputhost.exe",
    "lockapp.exe",
    "sihost.exe",
    "ctfmon.exe",
    "lag.exe",
];
pub(super) const NON_GAME: &[&str] = &[
    "chrome.exe",
    "msedge.exe",
    "firefox.exe",
    "librewolf.exe",
    "brave.exe",
    "opera.exe",
    "opera_gx.exe",
    "vivaldi.exe",
    "browser.exe",
    "arc.exe",
    "discord.exe",
    "discordcanary.exe",
    "discordptb.exe",
    "slack.exe",
    "ms-teams.exe",
    "teams.exe",
    "spotify.exe",
    "code.exe",
    "claude.exe",
    "notion.exe",
    "whatsapp.exe",
    "telegram.exe",
    "obs64.exe",
    "obs32.exe",
    "obs.exe",
    "streamlabs obs.exe",
    "xsplit.core.exe",
    "vlc.exe",
    "mpc-hc64.exe",
    "mpc-hc.exe",
    "mpc-be64.exe",
    "potplayermini64.exe",
    "potplayer64.exe",
    "mpv.exe",
    "wmplayer.exe",
    "steam.exe",
    "steamwebhelper.exe",
    "epicgameslauncher.exe",
    "battle.net.exe",
    "galaxyclient.exe",
    "eadesktop.exe",
    "ealauncher.exe",
    "riotclientux.exe",
    "riotclientservices.exe",
];

/// File name without ".exe" (any case).
pub(super) fn stem(exe: &str) -> &str {
    if exe.len() > 4 && exe[exe.len() - 4..].eq_ignore_ascii_case(".exe") {
        &exe[..exe.len() - 4]
    } else {
        exe
    }
}

/// The display name of a game on the known list.
pub(super) fn known_name(exe: &str) -> Option<&'static str> {
    let stem = stem(exe);
    KNOWN.iter().find(|(key, _)| key.eq_ignore_ascii_case(stem)).map(|(_, name)| *name)
}

pub(super) fn is_engine_class(class: &str) -> bool {
    ENGINES.iter().any(|c| c.eq_ignore_ascii_case(class))
}

/// The shell, Lag itself, browsers, chat / media apps and launchers: never recorded as a game.
pub(super) fn is_excluded(exe: &str) -> bool {
    SHELL.iter().chain(NON_GAME).any(|s| s.eq_ignore_ascii_case(exe))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_and_classes() {
        assert_eq!(known_name("CS2.EXE"), Some("Counter-Strike 2"));
        assert_eq!(known_name("cs2"), Some("Counter-Strike 2"));
        assert_eq!(known_name("ChatGPT.exe"), None);
        assert_eq!(stem("Game.EXE"), "Game");
        assert_eq!(stem(".exe"), ".exe");
        assert!(is_engine_class("unitywndclass") && !is_engine_class("Chrome_WidgetWin_1"));
        assert!(is_excluded("DISCORD.EXE") && is_excluded("lag.exe") && !is_excluded("cs2.exe"));
    }
}
