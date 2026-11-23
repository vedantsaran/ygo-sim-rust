use serde::{Deserialize, Serialize};
use neon_ygo_core::*;
use crate::banlist::Banlist;
use crate::catalog::CardDatabase;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SortBy {
    #[default]
    NameAsc,
    AtkDesc,
    DefDesc,
    LevelDesc,
    CodeAsc,
}

/// Multi-criteria Deck Builder search filter (1:1 YGO Omega Deck Builder).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeckFilter {
    pub text: Option<String>,
    pub card_type_any: Option<CardType>,
    pub card_type_all: Option<CardType>,
    pub attribute: Option<Attribute>,
    pub race: Option<Race>,
    pub level_min: Option<u32>,
    pub level_max: Option<u32>,
    pub scale_min: Option<u32>,
    pub scale_max: Option<u32>,
    pub atk_min: Option<i32>,
    pub atk_max: Option<i32>,
    pub def_min: Option<i32>,
    pub def_max: Option<i32>,
    pub link_markers_all: Option<LinkMarker>,
    pub setcode: Option<u16>,
    pub max_banlist_limit: Option<u8>,
    pub max_genesys_points: Option<u32>,
    pub sort: SortBy,
}

impl DeckFilter {
    pub fn search<'a>(&self, db: &'a CardDatabase, banlist: Option<&Banlist>) -> Vec<&'a CardData> {
        let q = self.text.as_ref().map(|s| s.to_lowercase());
        let mut results: Vec<&CardData> = db
            .cards
            .values()
            .filter(|c| {
                if let Some(ref text) = q {
                    if !text.is_empty()
                        && !c.name.to_lowercase().contains(text)
                        && !c.desc.to_lowercase().contains(text)
                        && !c.code.to_string().contains(text)
                    {
                        return false;
                    }
                }
                if let Some(any) = self.card_type_any {
                    if !c.card_type.intersects(any) {
                        return false;
                    }
                }
                if let Some(all) = self.card_type_all {
                    if !c.card_type.contains(all) {
                        return false;
                    }
                }
                if let Some(attr) = self.attribute {
                    if !c.attribute.intersects(attr) {
                        return false;
                    }
                }
                if let Some(race) = self.race {
                    if !c.race.intersects(race) {
                        return false;
                    }
                }
                if let Some(lo) = self.level_min {
                    if c.level < lo {
                        return false;
                    }
                }
                if let Some(hi) = self.level_max {
                    if c.level > hi {
                        return false;
                    }
                }
                if let Some(lo) = self.scale_min {
                    if !c.is_pendulum() || c.lscale < lo {
                        return false;
                    }
                }
                if let Some(hi) = self.scale_max {
                    if !c.is_pendulum() || c.rscale > hi {
                        return false;
                    }
                }
                if let Some(lo) = self.atk_min {
                    if !c.is_monster() || c.atk < lo {
                        return false;
                    }
                }
                if let Some(hi) = self.atk_max {
                    if !c.is_monster() || c.atk > hi {
                        return false;
                    }
                }
                if let Some(lo) = self.def_min {
                    if !c.is_monster() || c.is_link() || c.def < lo {
                        return false;
                    }
                }
                if let Some(hi) = self.def_max {
                    if !c.is_monster() || c.is_link() || c.def > hi {
                        return false;
                    }
                }
                if let Some(lm) = self.link_markers_all {
                    if !c.link_markers.contains(lm) {
                        return false;
                    }
                }
                if let Some(sc) = self.setcode {
                    if !c.is_set_card(sc) {
                        return false;
                    }
                }
                if let (Some(max_lim), Some(bl)) = (self.max_banlist_limit, banlist) {
                    if bl.max_copies(c.effective_alias()) > max_lim {
                        return false;
                    }
                }
                if let Some(max_pts) = self.max_genesys_points {
                    let pts = banlist
                        .and_then(|bl| bl.genesys_point_overrides.get(&c.effective_alias()).copied())
                        .unwrap_or(c.genesys_points);
                    if pts > max_pts {
                        return false;
                    }
                }
                true
            })
            .collect();

        match self.sort {
            SortBy::NameAsc => results.sort_by(|a, b| a.name.cmp(&b.name)),
            SortBy::AtkDesc => results.sort_by(|a, b| b.atk.cmp(&a.atk).then_with(|| a.name.cmp(&b.name))),
            SortBy::DefDesc => results.sort_by(|a, b| b.def.cmp(&a.def).then_with(|| a.name.cmp(&b.name))),
            SortBy::LevelDesc => results.sort_by(|a, b| b.level.cmp(&a.level).then_with(|| a.name.cmp(&b.name))),
            SortBy::CodeAsc => results.sort_by_key(|c| c.code),
        }
        results
    }
}

/// Exact Hypergeometric Hand Probability Calculator (YGO Omega Deck Tester tool!).
/// Computes `P(X >= min_successes)` when drawing `hand_size` cards without replacement from
/// `deck_size` containing `success_copies` copies of desired cards.
pub fn hypergeometric_at_least(
    deck_size: usize,
    success_copies: usize,
    hand_size: usize,
    min_successes: usize,
) -> f64 {
    if min_successes == 0 {
        return 1.0;
    }
    if hand_size > deck_size || success_copies > deck_size || min_successes > success_copies || min_successes > hand_size {
        return 0.0;
    }
    let denom = comb(deck_size, hand_size);
    if denom == 0.0 {
        return 0.0;
    }
    let max_k = success_copies.min(hand_size);
    let mut prob = 0.0;
    for k in min_successes..=max_k {
        let ways = comb(success_copies, k) * comb(deck_size - success_copies, hand_size - k);
        prob += ways / denom;
    }
    prob.clamp(0.0, 1.0)
}

fn comb(n: usize, k: usize) -> f64 {
    if k > n {
        return 0.0;
    }
    let k = k.min(n - k);
    let mut res = 1.0f64;
    for i in 1..=k {
        res *= (n - k + i) as f64 / (i as f64);
    }
    res
}
