use std::collections::{BTreeSet, VecDeque};
use serde::{Deserialize, Serialize};
use crate::constants::*;

/// Official Yu-Gi-Oh! Master Rules revisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MasterRule {
    /// Original Duel Monsters / GX (1 shared Field Spell, Turn 1 draw, Ignition priority)
    Mr1 = 1,
    /// Synchro / Xyz era (5Ds / Zexal)
    Mr2 = 2,
    /// Pendulum era (ARC-V: 2 separate Field Spells, Turn 1 skip draw, separate Pendulum Zones)
    Mr3 = 3,
    /// Link era (VRAINS: 2 EMZs, Pendulum in S/T zones 0 & 4, ALL Extra Deck monsters need EMZ/Link)
    Mr4 = 4,
    /// 2020 Revision / Current (Fusion/Synchro/Xyz free to MMZ; Link & face-up Pendulum need EMZ/Link)
    Mr5 = 5,
}

/// All YGO Omega supported game formats / presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum FormatPreset {
    TcgAdvanced,
    Ocg,
    MasterDuel,
    Goat,
    Edison,
    SpeedDuel,
    RushDuel,
    TurboDuel,
    ActionDuel,
    TagDuel,
    RelayDuel,
    Genesys,
    Custom,
}

/// Complete Duel Configuration matching YGO Omega room/duel setup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuelConfig {
    pub format: FormatPreset,
    pub master_rule: MasterRule,
    pub flags: DuelFlags,
    pub starting_lp: i32,
    pub starting_hand: usize,
    pub draw_per_turn: usize,
    pub hand_size_limit: usize,
    pub max_main_deck: usize,
    pub min_main_deck: usize,
    pub max_extra_deck: usize,
    pub max_side_deck: usize,
    pub turn_time_limit_secs: u32,
    pub extra_time_per_turn_secs: u32,
    pub genesys_point_limit: u32,
    pub seed: u32,
}

impl DuelConfig {
    pub fn from_preset(preset: FormatPreset, seed: u32) -> Self {
        match preset {
            FormatPreset::TcgAdvanced | FormatPreset::Ocg | FormatPreset::MasterDuel | FormatPreset::Custom => Self {
                format: preset,
                master_rule: MasterRule::Mr5,
                flags: DuelFlags::EXTRA_MONSTER_ZONES,
                starting_lp: 8000,
                starting_hand: 5,
                draw_per_turn: 1,
                hand_size_limit: 6,
                min_main_deck: 40,
                max_main_deck: 60,
                max_extra_deck: 15,
                max_side_deck: 15,
                turn_time_limit_secs: 240,
                extra_time_per_turn_secs: 60,
                genesys_point_limit: 0,
                seed,
            },
            FormatPreset::Goat => Self {
                format: preset,
                master_rule: MasterRule::Mr1,
                flags: DuelFlags::OBSOLETE_IGNITION | DuelFlags::TURN1_DRAW | DuelFlags::SHARED_FIELD_SPELL,
                starting_lp: 8000,
                starting_hand: 5,
                draw_per_turn: 1,
                hand_size_limit: 6,
                min_main_deck: 40,
                max_main_deck: 80,
                max_extra_deck: 100, // Unlimited Fusion Deck in 2005 GOAT!
                max_side_deck: 15,
                turn_time_limit_secs: 240,
                extra_time_per_turn_secs: 60,
                genesys_point_limit: 0,
                seed,
            },
            FormatPreset::Edison => Self {
                format: preset,
                master_rule: MasterRule::Mr2,
                flags: DuelFlags::OBSOLETE_IGNITION | DuelFlags::TURN1_DRAW | DuelFlags::SHARED_FIELD_SPELL,
                starting_lp: 8000,
                starting_hand: 5,
                draw_per_turn: 1,
                hand_size_limit: 6,
                min_main_deck: 40,
                max_main_deck: 60,
                max_extra_deck: 15,
                max_side_deck: 15,
                turn_time_limit_secs: 240,
                extra_time_per_turn_secs: 60,
                genesys_point_limit: 0,
                seed,
            },
            FormatPreset::SpeedDuel => Self {
                format: preset,
                master_rule: MasterRule::Mr5,
                flags: DuelFlags::SPEED_DUEL | DuelFlags::THREE_COLUMNS_FIELD | DuelFlags::NO_MAIN_PHASE_2,
                starting_lp: 4000,
                starting_hand: 4,
                draw_per_turn: 1,
                hand_size_limit: 6,
                min_main_deck: 20,
                max_main_deck: 30,
                max_extra_deck: 6,
                max_side_deck: 6,
                turn_time_limit_secs: 180,
                extra_time_per_turn_secs: 30,
                genesys_point_limit: 0,
                seed,
            },
            FormatPreset::RushDuel => Self {
                format: preset,
                master_rule: MasterRule::Mr5,
                flags: DuelFlags::RUSH_DUEL | DuelFlags::THREE_COLUMNS_FIELD | DuelFlags::NO_MAIN_PHASE_2 | DuelFlags::TURN1_DRAW,
                starting_lp: 8000,
                starting_hand: 4,
                draw_per_turn: 5, // Draw until 5 (or 1 if >=5)
                hand_size_limit: 99, // No hand size limit in Rush Duel!
                min_main_deck: 40,
                max_main_deck: 60,
                max_extra_deck: 15,
                max_side_deck: 15,
                turn_time_limit_secs: 180,
                extra_time_per_turn_secs: 30,
                genesys_point_limit: 0,
                seed,
            },
            FormatPreset::TurboDuel => Self {
                format: preset,
                master_rule: MasterRule::Mr5,
                flags: DuelFlags::EXTRA_MONSTER_ZONES | DuelFlags::TURBO_DUEL,
                starting_lp: 8000,
                starting_hand: 5,
                draw_per_turn: 1,
                hand_size_limit: 6,
                min_main_deck: 40,
                max_main_deck: 60,
                max_extra_deck: 15,
                max_side_deck: 15,
                turn_time_limit_secs: 240,
                extra_time_per_turn_secs: 60,
                genesys_point_limit: 0,
                seed,
            },
            FormatPreset::ActionDuel => Self {
                format: preset,
                master_rule: MasterRule::Mr5,
                flags: DuelFlags::EXTRA_MONSTER_ZONES | DuelFlags::ACTION_DUEL,
                starting_lp: 8000,
                starting_hand: 5,
                draw_per_turn: 1,
                hand_size_limit: 6,
                min_main_deck: 40,
                max_main_deck: 60,
                max_extra_deck: 15,
                max_side_deck: 15,
                turn_time_limit_secs: 240,
                extra_time_per_turn_secs: 60,
                genesys_point_limit: 0,
                seed,
            },
            FormatPreset::TagDuel => Self {
                format: preset,
                master_rule: MasterRule::Mr5,
                flags: DuelFlags::EXTRA_MONSTER_ZONES | DuelFlags::TAG_MODE,
                starting_lp: 16000,
                starting_hand: 5,
                draw_per_turn: 1,
                hand_size_limit: 6,
                min_main_deck: 40,
                max_main_deck: 60,
                max_extra_deck: 15,
                max_side_deck: 15,
                turn_time_limit_secs: 240,
                extra_time_per_turn_secs: 60,
                genesys_point_limit: 0,
                seed,
            },
            FormatPreset::RelayDuel => Self {
                format: preset,
                master_rule: MasterRule::Mr5,
                flags: DuelFlags::EXTRA_MONSTER_ZONES | DuelFlags::RELAY_MODE,
                starting_lp: 8000,
                starting_hand: 5,
                draw_per_turn: 1,
                hand_size_limit: 6,
                min_main_deck: 40,
                max_main_deck: 60,
                max_extra_deck: 15,
                max_side_deck: 15,
                turn_time_limit_secs: 240,
                extra_time_per_turn_secs: 60,
                genesys_point_limit: 0,
                seed,
            },
            FormatPreset::Genesys => Self {
                format: preset,
                master_rule: MasterRule::Mr5,
                flags: DuelFlags::GENESYS_MODE | DuelFlags::NO_PENDULUM_OR_LINK,
                starting_lp: 8000,
                starting_hand: 5,
                draw_per_turn: 1,
                hand_size_limit: 6,
                min_main_deck: 40,
                max_main_deck: 60,
                max_extra_deck: 15,
                max_side_deck: 15,
                turn_time_limit_secs: 240,
                extra_time_per_turn_secs: 60,
                genesys_point_limit: 100,
                seed,
            },
        }
    }

    pub fn with_master_rule(mut self, mr: MasterRule) -> Self {
        self.master_rule = mr;
        match mr {
            MasterRule::Mr1 | MasterRule::Mr2 => {
                self.flags.insert(DuelFlags::TURN1_DRAW | DuelFlags::SHARED_FIELD_SPELL);
                self.flags.remove(DuelFlags::EXTRA_MONSTER_ZONES | DuelFlags::MR4_EXTRA_LOCK | DuelFlags::SEPARATE_PZONE);
            }
            MasterRule::Mr3 => {
                self.flags.insert(DuelFlags::SEPARATE_PZONE);
                self.flags.remove(DuelFlags::TURN1_DRAW | DuelFlags::SHARED_FIELD_SPELL | DuelFlags::EXTRA_MONSTER_ZONES | DuelFlags::MR4_EXTRA_LOCK);
            }
            MasterRule::Mr4 => {
                self.flags.insert(DuelFlags::EXTRA_MONSTER_ZONES | DuelFlags::MR4_EXTRA_LOCK);
                self.flags.remove(DuelFlags::TURN1_DRAW | DuelFlags::SHARED_FIELD_SPELL | DuelFlags::SEPARATE_PZONE);
            }
            MasterRule::Mr5 => {
                self.flags.insert(DuelFlags::EXTRA_MONSTER_ZONES);
                self.flags.remove(DuelFlags::TURN1_DRAW | DuelFlags::SHARED_FIELD_SPELL | DuelFlags::SEPARATE_PZONE | DuelFlags::MR4_EXTRA_LOCK);
            }
        }
        self
    }

    pub fn mmz_count(&self) -> usize {
        if self.flags.contains(DuelFlags::THREE_COLUMNS_FIELD) { 3 } else { 5 }
    }

    pub fn stz_count(&self) -> usize {
        if self.flags.contains(DuelFlags::THREE_COLUMNS_FIELD) { 3 } else { 5 }
    }

    pub fn has_emz(&self) -> bool {
        self.flags.contains(DuelFlags::EXTRA_MONSTER_ZONES)
    }

    pub fn is_rush(&self) -> bool {
        self.flags.contains(DuelFlags::RUSH_DUEL)
    }

    pub fn is_speed(&self) -> bool {
        self.flags.contains(DuelFlags::SPEED_DUEL)
    }

    /// Left/Right Pendulum Zone sequence indices in `SZONE` (or dedicated `PZONE`).
    pub fn pendulum_zone_seqs(&self) -> (u8, u8) {
        if self.flags.contains(DuelFlags::SEPARATE_PZONE) {
            (6, 7)
        } else if self.flags.contains(DuelFlags::THREE_COLUMNS_FIELD) {
            (0, 2)
        } else {
            (0, 4)
        }
    }

    /// Does a monster being Special Summoned from the Extra Deck require an EMZ or Linked MMZ?
    pub fn extra_deck_requires_emz_or_link(&self, card_type: CardType) -> bool {
        if !self.has_emz() {
            return false;
        }
        if self.flags.contains(DuelFlags::MR4_EXTRA_LOCK) || self.master_rule == MasterRule::Mr4 {
            return true;
        }
        // Master Rule 5 (2020 Revision): Link Monsters and face-up Pendulum Monsters from Extra Deck require EMZ/Linked MMZ.
        card_type.contains(CardType::LINK) || card_type.contains(CardType::PENDULUM)
    }
}

/// Unified 2-player battlefield coordinate (`row 0 = P0 MMZ`, `row 1 = EMZ`, `row 2 = P1 MMZ`; `col 0..=4`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GridPos {
    pub row: i8,
    pub col: i8,
}

/// Convert `(controller, monster_zone_seq)` into unified `GridPos`.
/// `seq 0..5` = MMZ; `seq 5` = controller's Left EMZ; `seq 6` = controller's Right EMZ.
pub fn mzone_to_grid(controller: PlayerId, seq: u8) -> Option<GridPos> {
    let team = controller % 2;
    if team == 0 {
        match seq {
            0..=4 => Some(GridPos { row: 0, col: seq as i8 }),
            5 => Some(GridPos { row: 1, col: 1 }),
            6 => Some(GridPos { row: 1, col: 3 }),
            _ => None,
        }
    } else {
        match seq {
            0..=4 => Some(GridPos { row: 2, col: 4 - seq as i8 }),
            5 => Some(GridPos { row: 1, col: 3 }),
            6 => Some(GridPos { row: 1, col: 1 }),
            _ => None,
        }
    }
}

/// Convert unified `GridPos` back to `(owning_team, seq)` for MMZ, or shared EMZ `(0, 5/6)` / `(1, 6/5)`.
pub fn grid_to_mzone(pos: GridPos) -> Option<(PlayerId, u8)> {
    match (pos.row, pos.col) {
        (0, c @ 0..=4) => Some((0, c as u8)),
        (2, c @ 0..=4) => Some((1, (4 - c) as u8)),
        (1, 1) => Some((0, 5)), // P0 Left EMZ == P1 Right EMZ
        (1, 3) => Some((0, 6)), // P0 Right EMZ == P1 Left EMZ
        _ => None,
    }
}

/// Returns the unified `GridPos` targets pointed to by a Link Monster's `LinkMarker`s.
pub fn pointed_grid_positions(controller: PlayerId, seq: u8, markers: LinkMarker) -> Vec<GridPos> {
    let Some(origin) = mzone_to_grid(controller, seq) else {
        return Vec::new();
    };
    let dir_sign: i8 = if controller % 2 == 0 { 1 } else { -1 };
    let offsets: &[(LinkMarker, i8, i8)] = &[
        (LinkMarker::TOP_LEFT,      1, -1),
        (LinkMarker::TOP,           1,  0),
        (LinkMarker::TOP_RIGHT,     1,  1),
        (LinkMarker::LEFT,          0, -1),
        (LinkMarker::RIGHT,         0,  1),
        (LinkMarker::BOTTOM_LEFT,  -1, -1),
        (LinkMarker::BOTTOM,       -1,  0),
        (LinkMarker::BOTTOM_RIGHT, -1,  1),
    ];

    let mut out = Vec::new();
    for &(flag, dr, dc) in offsets {
        if markers.contains(flag) {
            let nr = origin.row + dr * dir_sign;
            let nc = origin.col + dc * dir_sign;
            let candidate = GridPos { row: nr, col: nc };
            if grid_to_mzone(candidate).is_some() {
                out.push(candidate);
            }
        }
    }
    out
}

/// Check if two Link Monsters at `a` and `b` mutually point to each other (Co-Linked!).
pub fn are_co_linked(
    ctrl_a: PlayerId,
    seq_a: u8,
    markers_a: LinkMarker,
    ctrl_b: PlayerId,
    seq_b: u8,
    markers_b: LinkMarker,
) -> bool {
    let (Some(pos_a), Some(pos_b)) = (mzone_to_grid(ctrl_a, seq_a), mzone_to_grid(ctrl_b, seq_b)) else {
        return false;
    };
    let a_points_b = pointed_grid_positions(ctrl_a, seq_a, markers_a).contains(&pos_b);
    let b_points_a = pointed_grid_positions(ctrl_b, seq_b, markers_b).contains(&pos_a);
    a_points_b && b_points_a
}

/// Given all face-up Link Monsters `(controller, seq, link_markers)` currently on the field,
/// check whether a continuous Co-Link chain connects EMZ Left `(row 1, col 1)` and EMZ Right `(row 1, col 3)`
/// (Extra Link / U-Link!). Also returns all `GridPos` nodes that are part of an Extra Link.
pub fn extra_linked_positions(links: &[(PlayerId, u8, LinkMarker)]) -> BTreeSet<GridPos> {
    let emz_left = GridPos { row: 1, col: 1 };
    let emz_right = GridPos { row: 1, col: 3 };

    let mut nodes: Vec<(GridPos, PlayerId, u8, LinkMarker)> = Vec::new();
    for &(ctrl, seq, markers) in links {
        if let Some(pos) = mzone_to_grid(ctrl, seq) {
            nodes.push((pos, ctrl, seq, markers));
        }
    }

    let has_left = nodes.iter().any(|n| n.0 == emz_left);
    let has_right = nodes.iter().any(|n| n.0 == emz_right);
    if !has_left || !has_right {
        return BTreeSet::new();
    }

    // BFS over Co-Link edges starting from emz_left
    let mut visited = BTreeSet::new();
    let mut queue = VecDeque::new();
    visited.insert(emz_left);
    queue.push_back(emz_left);

    while let Some(curr) = queue.pop_front() {
        let Some(&(_, ctrl_a, seq_a, markers_a)) = nodes.iter().find(|n| n.0 == curr) else {
            continue;
        };
        for &(pos_b, ctrl_b, seq_b, markers_b) in &nodes {
            if !visited.contains(&pos_b)
                && are_co_linked(ctrl_a, seq_a, markers_a, ctrl_b, seq_b, markers_b)
            {
                visited.insert(pos_b);
                queue.push_back(pos_b);
            }
        }
    }

    if visited.contains(&emz_right) {
        visited
    } else {
        BTreeSet::new()
    }
}

/// Check if placing a Link Monster with `new_markers` into the other EMZ (`target_emz_seq`, 5 or 6)
/// completes an Extra Link (U-Link), allowing one player to occupy BOTH Extra Monster Zones!
pub fn would_complete_extra_link(
    existing_links: &[(PlayerId, u8, LinkMarker)],
    player: PlayerId,
    target_emz_seq: u8,
    new_markers: LinkMarker,
) -> bool {
    let mut test = existing_links.to_vec();
    test.push((player, target_emz_seq, new_markers));
    !extra_linked_positions(&test).is_empty()
}
