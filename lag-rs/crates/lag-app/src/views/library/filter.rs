//! Search text + game chips -> the visible clips.

use super::clip::*;
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChipModel {
    pub label: String,
    pub filter_key: String,
    pub is_active: bool,
}

/// Rebuilds category chips from ALL clips and clears stale active filter (LibraryViewModel.cs:76-94).
pub fn build_game_chips(
    clips: &[ClipEntry],
    active_filter: &mut Option<String>,
    desktop_label: &str,
) -> Vec<ChipModel> {
    let mut seen = HashSet::<String>::new();
    let mut games = Vec::<String>::new();
    for clip in clips {
        if clip.has_game() {
            if let Some(ref g) = clip.game {
                let lower = g.to_lowercase();
                if seen.insert(lower) {
                    games.push(g.clone());
                }
            }
        }
    }
    games.sort_by_key(|g| g.to_lowercase());
    let desktop_present = clips.iter().any(|c| !c.has_game());

    if let Some(ref current) = *active_filter {
        if current == DESKTOP_FILTER_KEY {
            if !desktop_present {
                *active_filter = None;
            }
        } else if !games.iter().any(|g| eq_ignore_case(g, current)) {
            *active_filter = None;
        }
    }

    let mut chips = Vec::with_capacity(games.len() + usize::from(desktop_present));
    for g in games {
        let active = active_filter.as_deref().is_some_and(|f| eq_ignore_case(&g, f));
        chips.push(ChipModel { label: g.clone(), filter_key: g, is_active: active });
    }
    if desktop_present {
        chips.push(ChipModel {
            label: desktop_label.to_string(),
            filter_key: DESKTOP_FILTER_KEY.to_string(),
            is_active: active_filter.as_deref() == Some(DESKTOP_FILTER_KEY),
        });
    }
    chips
}

/// Filters clips with AND of favorites, game/Desktop category, and trimmed search (LibraryViewModel.cs:97-112).
pub fn filter_clips<'a>(
    clips: &'a [ClipEntry],
    favorites_only: bool,
    game_filter: Option<&str>,
    search_text: &str,
) -> Vec<(usize, &'a ClipEntry)> {
    let q = search_text.trim();
    clips
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            if favorites_only && !c.is_favorite {
                return false;
            }
            match game_filter {
                Some(DESKTOP_FILTER_KEY) => {
                    if c.has_game() {
                        return false;
                    }
                }
                Some(game) if !c.game.as_deref().is_some_and(|g| eq_ignore_case(g, game)) => return false,
                Some(_) => {}
                None => {}
            }
            if !q.is_empty() {
                let name_match = contains_ignore_case(c.display_name(), q);
                let game_match = c.game.as_deref().is_some_and(|g| contains_ignore_case(g, q));
                if !name_match && !game_match {
                    return false;
                }
            }
            true
        })
        .collect()
}
