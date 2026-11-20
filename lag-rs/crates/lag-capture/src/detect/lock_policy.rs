//! When a running game lock gives way. A lock used to be final while its window lived: the engine took the
//! maximized ChatGPT window for a game and stayed on it for hours while the owner played CS2 (2026-11-20).
//! Now, about every 4 s:
//! * a certain game takes over from a guess, and any game 20 000 points ahead takes over anyway;
//! * a guessed lock that no longer looks like a game at all goes back to the desktop.
//!
//! A change has to be seen on two checks in a row, so alt-tabbing or a loading screen does not flip the
//! source. A certain game that stops qualifying for a moment (minimized, alt-tabbed) keeps its lock.
//! Pure: the engine scans, this decides, the engine applies.
use super::GameCandidate;

/// Points a challenger must lead by when both are equally sure.
const MARGIN: i32 = 20_000;
/// Checks in a row a change must hold.
const CHECKS: u32 = 2;
/// The engine polls every ~2 s; a lock is reviewed on every second poll.
const EVERY: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockChange {
    SwitchTo(isize),
    BackToDesktop,
}

#[derive(Default)]
pub struct LockReview {
    polls: u32,
    pending: Option<(LockChange, u32)>,
}

impl LockReview {
    /// Whether this poll reviews the lock (scanning every window costs a little; this runs for a whole game).
    pub fn due(&mut self) -> bool {
        self.polls = self.polls.wrapping_add(1);
        self.polls.is_multiple_of(EVERY)
    }

    /// `locked`: the window recorded now; `locked_certain`: whether it was a certain game when it was locked
    /// (used when it does not qualify right now); `candidates`: every window that qualifies now.
    pub fn review(&mut self, locked: isize, locked_certain: bool, candidates: &[GameCandidate]) -> Option<LockChange> {
        let current = candidates.iter().find(|c| c.hwnd == locked);
        let best = candidates.iter().filter(|c| c.hwnd != locked).max_by_key(|c| (c.certain, c.score));
        let wanted = match (best, current) {
            (Some(b), Some(c))
                if (b.certain && !c.certain) || (b.certain == c.certain && b.score >= c.score + MARGIN) =>
            {
                Some(LockChange::SwitchTo(b.hwnd))
            }
            (Some(b), None) if !locked_certain || b.certain => Some(LockChange::SwitchTo(b.hwnd)),
            (None, None) if !locked_certain => Some(LockChange::BackToDesktop),
            _ => None,
        };
        let Some(change) = wanted else {
            self.pending = None;
            return None;
        };
        let seen = match self.pending {
            Some((c, n)) if c == change => n + 1,
            _ => 1,
        };
        if seen < CHECKS {
            self.pending = Some((change, seen));
            return None;
        }
        self.pending = None;
        Some(change)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(hwnd: isize, score: i32, certain: bool) -> GameCandidate {
        GameCandidate {
            hwnd,
            pid: hwnd as u32,
            exe: format!("{hwnd}.exe"),
            game_name: String::new(),
            score,
            fullscreen: true,
            certain,
        }
    }

    #[test]
    fn a_real_game_takes_over_a_guess_after_two_checks() {
        let mut r = LockReview::default();
        let now = [cand(1, 78_000, false), cand(2, 170_000, true)]; // 1 = ChatGPT guessed, 2 = CS2
        assert_eq!(r.review(1, false, &now), None, "first sighting only");
        assert_eq!(r.review(1, false, &now), Some(LockChange::SwitchTo(2)));
    }

    #[test]
    fn a_flicker_does_not_switch() {
        let mut r = LockReview::default();
        let challenger = [cand(1, 80_000, true), cand(2, 150_000, true)];
        let calm = [cand(1, 80_000, true)];
        assert_eq!(r.review(1, true, &challenger), None);
        assert_eq!(r.review(1, true, &calm), None, "gone again: the count starts over");
        assert_eq!(r.review(1, true, &challenger), None);
        assert_eq!(r.review(1, true, &challenger), Some(LockChange::SwitchTo(2)));
    }

    #[test]
    fn close_scores_and_guesses_do_not_unseat_a_certain_game() {
        let mut r = LockReview::default();
        let close = [cand(1, 100_000, true), cand(2, 110_000, true)];
        let guess = [cand(1, 100_000, true), cand(3, 200_000, false)];
        for _ in 0..4 {
            assert_eq!(r.review(1, true, &close), None);
            assert_eq!(r.review(1, true, &guess), None);
        }
    }

    #[test]
    fn a_certain_game_keeps_its_lock_while_alt_tabbed() {
        let mut r = LockReview::default();
        for _ in 0..4 {
            assert_eq!(r.review(1, true, &[]), None);
        }
    }

    #[test]
    fn a_guess_that_stopped_looking_like_a_game_goes_back_to_the_desktop() {
        let mut r = LockReview::default();
        assert_eq!(r.review(1, false, &[]), None);
        assert_eq!(r.review(1, false, &[]), Some(LockChange::BackToDesktop));
    }

    #[test]
    fn reviews_every_second_poll() {
        let mut r = LockReview::default();
        assert!(!r.due());
        assert!(r.due());
        assert!(!r.due());
    }
}
