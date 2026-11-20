//! The decision: is this window a game, how sure are we, and how strongly does it claim the recording.
//! Pure (facts in, verdict out), so every case below is a test.
//!
//! Certain (any one is enough): on our known list; on the Windows games list; installed by a game launcher
//! and actually in use (focused, fullscreen or loading the GPU; a launcher folder also holds tools).
//! Unsure games qualify only on two weak signals together: a game-engine window that is fullscreen or
//! focused, or a true fullscreen window (no title bar, on the recorded monitor) with a sustained 3D load.
//! Nothing else does: an app maximized next to a game, a chat window on another monitor, a short GPU
//! spike while an app animates (owner, 2026-11-20: ChatGPT recorded instead of CS2; Discord in Medal).
use super::catalog;

/// Points per signal: a certain source outweighs every weak signal, so a real game always wins a tie-break.
const KNOWN: i32 = 46_000;
const WINDOWS_GAME: i32 = 46_000;
const LAUNCHER: i32 = 30_000;
const SUSTAINED_GPU: i32 = 46_000;
const ENGINE: i32 = 30_000;
const FULLSCREEN: i32 = 30_000;
const FOCUSED: i32 = 18_000;

#[derive(Debug, Default, Clone, Copy)]
pub(super) struct Evidence<'a> {
    pub exe: &'a str,
    pub title: &'a str,
    pub windows_game: bool,
    pub launcher_install: bool,
    pub engine_window: bool,
    pub fullscreen: bool,
    pub focused: bool,
    pub sustained_gpu: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Verdict {
    pub name: String,
    pub score: i32,
    /// From a certain source (vs. a guess from weak signals).
    pub certain: bool,
}

pub(super) fn judge(e: &Evidence) -> Option<Verdict> {
    if catalog::is_excluded(e.exe) {
        return None;
    }
    let known = catalog::known_name(e.exe);
    let in_use = e.focused || e.fullscreen || e.sustained_gpu;
    let certain = known.is_some() || e.windows_game || (e.launcher_install && in_use);
    let guessed = (e.engine_window && (e.fullscreen || e.focused)) || (e.fullscreen && e.sustained_gpu);
    if !certain && !guessed {
        return None;
    }
    let score = [
        (known.is_some(), KNOWN),
        (e.windows_game, WINDOWS_GAME),
        (e.launcher_install, LAUNCHER),
        (e.sustained_gpu, SUSTAINED_GPU),
        (e.engine_window, ENGINE),
        (e.fullscreen, FULLSCREEN),
        (e.focused, FOCUSED),
    ]
    .iter()
    .filter(|(on, _)| *on)
    .map(|(_, points)| points)
    .sum();
    let name = match known {
        Some(n) => n.to_owned(),
        None if !e.title.trim().is_empty() => e.title.trim().to_owned(),
        None => catalog::stem(e.exe).to_owned(),
    };
    Some(Verdict { name, score, certain })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(exe: &'static str) -> Evidence<'static> {
        Evidence { exe, title: "", ..Default::default() }
    }

    #[test]
    fn real_games_are_certain_wherever_they_come_from() {
        // CS2: known + Windows list + Steam library, fullscreen, focused, GPU busy
        let cs2 = Evidence {
            windows_game: true,
            launcher_install: true,
            fullscreen: true,
            focused: true,
            sustained_gpu: true,
            ..ev("cs2.exe")
        };
        let v = judge(&cs2).unwrap();
        assert_eq!(v.name, "Counter-Strike 2");
        assert!(v.certain);
        // a game only Windows knows (not ours, not from a launcher), played windowed
        let v = judge(&Evidence { title: "Gnomium", windows_game: true, ..ev("Gnomium.exe") }).unwrap();
        assert!(v.certain);
        assert_eq!(v.name, "Gnomium");
        // a fresh Epic install Windows has not listed yet, while it is being played
        assert!(judge(&Evidence { launcher_install: true, focused: true, ..ev("NewGame.exe") }).unwrap().certain);
        // ... but a tool in a launcher folder sitting in the background is not
        assert!(judge(&Evidence { launcher_install: true, ..ev("ModTool.exe") }).is_none());
    }

    #[test]
    fn unknown_games_need_two_weak_signals() {
        // an indie game nobody lists: true fullscreen + sustained 3D load
        let v = judge(&Evidence { title: "Indie", fullscreen: true, sustained_gpu: true, ..ev("indie.exe") }).unwrap();
        assert!(!v.certain);
        assert_eq!(v.name, "Indie");
        // an engine window in focus
        assert!(judge(&Evidence { engine_window: true, focused: true, ..ev("ue_game.exe") }).is_some());
        // one weak signal alone is not enough
        for e in [
            Evidence { fullscreen: true, ..ev("x.exe") },
            Evidence { sustained_gpu: true, ..ev("x.exe") },
            Evidence { engine_window: true, ..ev("x.exe") },
            Evidence { focused: true, ..ev("x.exe") },
        ] {
            assert!(judge(&e).is_none(), "{e:?}");
        }
    }

    #[test]
    fn apps_never_take_the_recording() {
        // ChatGPT maximized and focused while CS2 runs: no fullscreen (it has a title bar), a GPU spike only
        assert!(judge(&Evidence { title: "ChatGPT", focused: true, ..ev("ChatGPT.exe") }).is_none());
        // Discord on another monitor, even with a sustained load (screen share / video)
        assert!(judge(&Evidence { sustained_gpu: true, ..ev("Discord.exe") }).is_none());
        // a browser playing a video fullscreen: excluded by name, whatever its signals
        assert!(judge(&Evidence { fullscreen: true, focused: true, sustained_gpu: true, ..ev("chrome.exe") }).is_none());
    }

    #[test]
    fn scores_add_up_per_signal() {
        let guess = judge(&Evidence {
            fullscreen: true,
            focused: true,
            sustained_gpu: true,
            engine_window: true,
            ..ev("x.exe")
        })
        .unwrap();
        assert_eq!(guess.score, SUSTAINED_GPU + ENGINE + FULLSCREEN + FOCUSED);
        assert!(!guess.certain, "weak signals never make a guess certain (ranking puts certain first)");
        assert_eq!(judge(&ev("cs2.exe")).unwrap().score, KNOWN);
    }
}
