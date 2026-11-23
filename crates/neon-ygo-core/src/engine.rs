use std::collections::{BTreeMap, BTreeSet};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::card::{CardData, CardId, CardInstance};
use crate::chain::{sort_segoc_triggers, ChainLink, ChainStack, PendingTrigger};
use crate::constants::*;
use crate::effect::{Effect, EffectCallback, EffectId, EffectValue};
use crate::events::{DuelEvent, DuelMessage};
use crate::manual::ManualCommand;
use crate::prompts::{ActivatableOption, Decision, Prompt};
use crate::rng::Mt19937;
use crate::rules::{
    are_co_linked, extra_linked_positions, mzone_to_grid, pointed_grid_positions,
    would_complete_extra_link, DuelConfig,
};

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DuelError {
    #[error("card {0:?} not found")]
    CardNotFound(CardId),
    #[error("effect {0} not found")]
    EffectNotFound(EffectId),
    #[error("illegal action: {0}")]
    IllegalAction(String),
    #[error("duel already ended")]
    DuelEnded,
    #[error("invalid zone {0}")]
    InvalidZone(u8),
    #[error("manual mode is not enabled")]
    ManualModeDisabled,
}

/// Pluggable script host trait implemented by `neon-ygo-lua` (or native test harnesses).
pub trait ScriptHost {
    /// Called once when a card is registered into the duel (executes `s.initial_effect(c)`).
    fn init_card(&mut self, _engine: &mut DuelEngine, _card_id: CardId) {}

    /// Evaluate an effect's `condition(e, tp, eg, ep, ev, re, r, rp)` callback.
    fn eval_condition(
        &mut self,
        _engine: &mut DuelEngine,
        _effect_id: EffectId,
        _tp: PlayerId,
        _event: Option<&DuelEvent>,
    ) -> bool {
        true
    }

    /// Check or pay an effect's `cost(e, tp, eg, ep, ev, re, r, rp, chk)` callback.
    fn eval_cost(
        &mut self,
        _engine: &mut DuelEngine,
        _effect_id: EffectId,
        _tp: PlayerId,
        _event: Option<&DuelEvent>,
        _chk: i32,
    ) -> bool {
        true
    }

    /// Check or select targets via `target(e, tp, eg, ep, ev, re, r, rp, chk)`.
    fn eval_target(
        &mut self,
        _engine: &mut DuelEngine,
        _effect_id: EffectId,
        _tp: PlayerId,
        _event: Option<&DuelEvent>,
        _chk: i32,
    ) -> bool {
        true
    }

    /// Execute an effect's `operation(e, tp, eg, ep, ev, re, r, rp)`.
    fn eval_operation(
        &mut self,
        _engine: &mut DuelEngine,
        _effect_id: EffectId,
        _tp: PlayerId,
        _event: Option<&DuelEvent>,
    ) {
    }

    /// Evaluate a dynamic `value(e, c)` callback for continuous effects.
    fn eval_value(
        &mut self,
        _engine: &mut DuelEngine,
        _effect_id: EffectId,
        _target_card: Option<CardId>,
    ) -> i64 {
        0
    }
}

/// Default no-op host when running purely with built-in Rust effects.
#[derive(Debug, Default)]
pub struct NoopScriptHost;
impl ScriptHost for NoopScriptHost {}

/// Per-player state inside a Duel (supports 2 players in standard, 4 players in Tag, or teams in Relay).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerState {
    pub id: PlayerId,
    pub name: String,
    pub lp: i32,
    pub normal_summons_used: u32,
    pub normal_summons_limit: u32,
    pub pendulum_summoned_this_turn: bool,
    pub chain_control: ChainControlMode,
    /// Hard-OPT codes used this turn (`code -> count`).
    pub opt_used_this_turn: BTreeMap<u32, u32>,
    /// Once-per-duel codes used (`code -> count`).
    pub opt_used_this_duel: BTreeMap<u32, u32>,
    /// Summons performed this turn (for Nibiru, Summon Limit, etc.).
    pub summons_this_turn: u32,
    /// Turbo Duel Speed Counters (0..=12).
    pub speed_counters: u32,
    /// Relay Duel reserve teammates' deck/extra lists (`(name, main, extra)`).
    pub relay_teammates: Vec<(String, Vec<CardData>, Vec<CardData>)>,
    /// Skill card used flag (Speed Duel).
    pub skill_used: bool,
}

impl PlayerState {
    pub fn new(id: PlayerId, name: impl Into<String>, lp: i32) -> Self {
        Self {
            id,
            name: name.into(),
            lp,
            normal_summons_used: 0,
            normal_summons_limit: 1,
            pendulum_summoned_this_turn: false,
            chain_control: ChainControlMode::Auto,
            opt_used_this_turn: BTreeMap::new(),
            opt_used_this_duel: BTreeMap::new(),
            summons_this_turn: 0,
            speed_counters: 0,
            relay_teammates: Vec::new(),
            skill_used: false,
        }
    }
}

/// Active battle state during Battle Phase.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BattleState {
    pub attacker: Option<CardId>,
    pub defender: Option<CardId>,
    pub attack_negated: bool,
    pub replay_available: bool,
    pub sub_step: DamageSubStep,
    pub expected_opp_monster_count: usize,
    pub destroyed_in_battle: Vec<CardId>,
    pub flipped_in_damage_step: Option<CardId>,
}

/// Active floodgate summary recalculated on every continuous effect pass.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalFloodgates {
    pub cannot_special_summon: [bool; 2],
    pub cannot_draw: [bool; 2],
    pub cannot_to_hand_from_deck: [bool; 2],
    pub cannot_remove: [bool; 2],
    pub to_grave_redirect_to_banish: [bool; 2],
    pub monsters_to_grave_redirect_to_banish: [bool; 2],
    pub cannot_activate_monster_effects: [bool; 2],
    pub cannot_activate_spells: [bool; 2],
    pub cannot_activate_traps: [bool; 2],
    pub extra_normal_summons: [u32; 2],
    pub hand_limit_override: [Option<usize>; 2],
}

/// Complete serializable snapshot of the duel state (used for Undo/Rollback, Replays, and Network Sync).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuelState {
    pub config: DuelConfig,
    pub rng: Mt19937,
    pub players: Vec<PlayerState>,
    /// Active player seats for teams `[team0_active_player, team1_active_player]` (for Tag Duel!).
    pub active_seat: [PlayerId; 2],
    /// Turn rotation index (0..2 for standard, 0..4 for Tag: 0 -> 2 -> 1 -> 3).
    pub tag_turn_slot: usize,
    pub turn_count: u32,
    pub turn_player: PlayerId,
    pub phase: Phase,
    pub entered_battle_phase: bool,
    pub cards: BTreeMap<CardId, CardInstance>,
    pub next_card_id: u32,
    pub effects: BTreeMap<EffectId, Effect>,
    pub next_effect_id: u32,
    pub effect_timestamp: u64,
    pub chain: ChainStack,
    pub battle: BattleState,
    pub floodgates: GlobalFloodgates,
    pub pending_events: Vec<DuelEvent>,
    pub last_resolved_event: Option<DuelEvent>,
    pub pending_triggers: Vec<PendingTrigger>,
    pub current_chain_targets: Vec<CardId>,
    pub manual_mode: bool,
    pub winner: Option<Option<PlayerId>>, // None = ongoing, Some(Some(p)) = winner p, Some(None) = draw
    pub win_reason: Option<WinReason>,
    pub action_counter: u64,
}

/// Complete 1:1 `ocgcore`-equivalent Yu-Gi-Oh! Duel Engine in 100% pure Rust.
#[derive(Debug, Clone)]
pub struct DuelEngine {
    pub state: DuelState,
    pub messages: Vec<DuelMessage>,
    pub snapshots: Vec<DuelState>,
}

impl DuelEngine {
    /// Create a new duel with two players (or 4 players in Tag Mode).
    pub fn new(config: DuelConfig, player0_name: impl Into<String>, player1_name: impl Into<String>) -> Self {
        let lp = config.starting_lp;
        let manual = config.flags.contains(DuelFlags::MANUAL_MODE);
        let rng = Mt19937::new(config.seed);
        let mut players = vec![
            PlayerState::new(0, player0_name, lp),
            PlayerState::new(1, player1_name, lp),
        ];
        if config.flags.contains(DuelFlags::TAG_MODE) {
            players.push(PlayerState::new(2, "Tag Partner 1", lp));
            players.push(PlayerState::new(3, "Tag Partner 2", lp));
        }
        let state = DuelState {
            config,
            rng,
            players,
            active_seat: [0, 1],
            tag_turn_slot: 0,
            turn_count: 0,
            turn_player: 0,
            phase: Phase::Draw,
            entered_battle_phase: false,
            cards: BTreeMap::new(),
            next_card_id: 1,
            effects: BTreeMap::new(),
            next_effect_id: 1,
            effect_timestamp: 1,
            chain: ChainStack::new(),
            battle: BattleState::default(),
            floodgates: GlobalFloodgates::default(),
            pending_events: Vec::new(),
            last_resolved_event: None,
            pending_triggers: Vec::new(),
            current_chain_targets: Vec::new(),
            manual_mode: manual,
            winner: None,
            win_reason: None,
            action_counter: 0,
        };
        Self {
            state,
            messages: Vec::new(),
            snapshots: Vec::new(),
        }
    }

    /// Configure Tag Duel teammate names (players 2 & 3).
    pub fn set_tag_partners(&mut self, partner0: impl Into<String>, partner1: impl Into<String>) {
        if self.state.players.len() >= 4 {
            self.state.players[2].name = partner0.into();
            self.state.players[3].name = partner1.into();
        }
    }

    /// Add a Relay Duel teammate to a team's reserve queue.
    pub fn add_relay_teammate(
        &mut self,
        team: PlayerId,
        name: impl Into<String>,
        main_deck: Vec<CardData>,
        extra_deck: Vec<CardData>,
    ) {
        let idx = (team % 2) as usize;
        if let Some(p) = self.state.players.get_mut(idx) {
            p.relay_teammates.push((name.into(), main_deck, extra_deck));
        }
    }

    /// Team index (0 or 1). In Tag Duel, P0 & P2 share Team 0; P1 & P3 share Team 1.
    pub fn team_of(&self, player: PlayerId) -> usize {
        (player % 2) as usize
    }

    pub fn opponent_of(&self, player: PlayerId) -> PlayerId {
        if self.state.config.flags.contains(DuelFlags::TAG_MODE) {
            self.state.active_seat[1 - self.team_of(player)]
        } else {
            1 - (player % 2)
        }
    }

    pub fn get_lp(&self, player: PlayerId) -> i32 {
        let idx = self.team_of(player);
        self.state.players[idx].lp
    }

    pub fn set_lp(&mut self, player: PlayerId, new_lp: i32) {
        let clamped = new_lp.max(0);
        let team = self.team_of(player);
        self.state.players[team].lp = clamped;
        if self.state.config.flags.contains(DuelFlags::TAG_MODE) && self.state.players.len() >= 4 {
            self.state.players[team + 2].lp = clamped;
        }
        self.emit(DuelMessage::LpUpdate {
            player: team as PlayerId,
            new_lp: clamped,
        });
        self.check_win_conditions();
    }

    pub fn set_chain_control(&mut self, player: PlayerId, mode: ChainControlMode) {
        if let Some(p) = self.state.players.get_mut(player as usize) {
            p.chain_control = mode;
        }
    }

    pub fn chain_control(&self, player: PlayerId) -> ChainControlMode {
        self.state
            .players
            .get(player as usize)
            .map(|p| p.chain_control)
            .unwrap_or(ChainControlMode::Auto)
    }

    pub fn is_finished(&self) -> bool {
        self.state.winner.is_some()
    }

    pub fn emit(&mut self, msg: DuelMessage) {
        self.messages.push(msg);
    }

    pub fn drain_messages(&mut self) -> Vec<DuelMessage> {
        std::mem::take(&mut self.messages)
    }

    /// Save a rollback snapshot (used before major actions / Manual Judge undo).
    pub fn save_snapshot(&mut self) {
        if self.snapshots.len() >= 128 {
            self.snapshots.remove(0);
        }
        self.snapshots.push(self.state.clone());
    }

    /// Add a card to a player's deck, extra deck, skill zone, or arbitrary starting zone (for Puzzles).
    pub fn add_card(
        &mut self,
        data: CardData,
        owner: PlayerId,
        location: Location,
        position: Position,
        host: &mut dyn ScriptHost,
    ) -> CardId {
        let id = CardId(self.state.next_card_id);
        self.state.next_card_id += 1;
        let seq = self.next_sequence_for(owner, location);
        let mut inst = CardInstance::new(id, data, owner, location, seq);
        inst.position = position;
        if location == Location::MZONE || location == Location::SZONE || location == Location::FZONE {
            inst.status.insert(CardStatus::EFFECT_ENABLED);
        }
        self.state.cards.insert(id, inst);
        host.init_card(self, id);
        id
    }

    /// Add a card at an exact zone sequence (used by Puzzles / `Debug.AddCard` / Manual Mode).
    pub fn add_card_at(
        &mut self,
        data: CardData,
        owner: PlayerId,
        controller: PlayerId,
        location: Location,
        sequence: u8,
        position: Position,
        proc_complete: bool,
        host: &mut dyn ScriptHost,
    ) -> CardId {
        let id = CardId(self.state.next_card_id);
        self.state.next_card_id += 1;
        let mut inst = CardInstance::new(id, data, owner, location, sequence);
        inst.controller = controller;
        inst.position = position;
        if proc_complete {
            inst.status.insert(CardStatus::PROC_COMPLETE);
        }
        if location.intersects(Location::ONFIELD | Location::FZONE | Location::PZONE) && position.intersects(Position::FACEUP) {
            inst.status.insert(CardStatus::EFFECT_ENABLED);
        }
        self.state.cards.insert(id, inst);
        host.init_card(self, id);
        id
    }

    /// Load a player's Main Deck, Extra Deck, and optional Skill Card.
    pub fn load_deck(
        &mut self,
        owner: PlayerId,
        main: Vec<CardData>,
        extra: Vec<CardData>,
        skill: Option<CardData>,
        host: &mut dyn ScriptHost,
    ) {
        for card in main {
            self.add_card(card, owner, Location::DECK, Position::FACEDOWN_DEFENSE, host);
        }
        for card in extra {
            self.add_card(card, owner, Location::EXTRA, Position::FACEDOWN_DEFENSE, host);
        }
        if let Some(sk) = skill {
            self.add_card(sk, owner, Location::SKILL, Position::FACEDOWN_DEFENSE, host);
        }
    }

    /// Register an `Effect` onto a card or globally onto a player.
    pub fn register_effect(&mut self, mut effect: Effect, host: &mut dyn ScriptHost) -> EffectId {
        let id = self.state.next_effect_id;
        self.state.next_effect_id += 1;
        effect.id = id;
        effect.creation_order = self.state.effect_timestamp;
        self.state.effect_timestamp += 1;
        if let Some(cid) = effect.handler_card {
            if let Some(card) = self.state.cards.get_mut(&cid) {
                card.effects.push(id);
            }
        }
        self.state.effects.insert(id, effect);
        self.recalculate_effects(host);
        id
    }

    /// Allocate a blank `Effect` builder ID (without registering yet).
    pub fn create_effect_struct(&mut self, owner_card: Option<CardId>, owner_player: PlayerId) -> Effect {
        let id = self.state.next_effect_id;
        self.state.next_effect_id += 1;
        let ts = self.state.effect_timestamp;
        self.state.effect_timestamp += 1;
        Effect::new(id, owner_card, owner_player, ts)
    }

    /// Start the duel: shuffle decks (unless `NO_SHUFFLE_DECK`), draw opening hands, begin Turn 1.
    pub fn start_duel(&mut self, first_player: PlayerId, host: &mut dyn ScriptHost) {
        self.state.turn_player = first_player;
        self.state.turn_count = 0;
        if !self.state.config.flags.contains(DuelFlags::NO_SHUFFLE_DECK) {
            let num_players = self.state.players.len() as PlayerId;
            for p in 0..num_players {
                self.shuffle_deck(p);
            }
        }

        let deck_counts = [
            self.cards_in(0, Location::DECK).len(),
            self.cards_in(1, Location::DECK).len(),
        ];
        let extra_counts = [
            self.cards_in(0, Location::EXTRA).len(),
            self.cards_in(1, Location::EXTRA).len(),
        ];

        self.emit(DuelMessage::Start {
            first_player,
            lp: [self.get_lp(0), self.get_lp(1)],
            deck_counts,
            extra_counts,
        });

        // Draw opening hands
        let start_hand = self.state.config.starting_hand;
        let num_players = self.state.players.len() as PlayerId;
        for p in 0..num_players {
            if !self.cards_in(p, Location::DECK).is_empty() && start_hand > 0 {
                self.draw_cards(p, start_hand, Reason::RULE, host);
            }
        }

        self.recalculate_effects(host);
        self.begin_turn(first_player, host);
    }

    /// Start a Puzzle / Custom board duel without shuffling or drawing opening hands.
    pub fn start_puzzle(&mut self, first_player: PlayerId, host: &mut dyn ScriptHost) {
        self.state.config.flags.insert(DuelFlags::NO_SHUFFLE_DECK);
        self.state.turn_player = first_player;
        self.state.turn_count = 1;
        self.state.phase = Phase::Main1;
        self.recalculate_effects(host);
        self.emit(DuelMessage::NewTurn {
            turn: 1,
            player: first_player,
        });
        self.emit(DuelMessage::NewPhase { phase: Phase::Main1 });
        self.check_win_conditions();
    }

    // -------------------------------------------------------------------------
    // ZONE QUERIES & LINK GEOMETRY
    // -------------------------------------------------------------------------

    pub fn cards_in(&self, controller: PlayerId, location: Location) -> Vec<CardId> {
        let team = self.team_of(controller);
        let is_tag = self.state.config.flags.contains(DuelFlags::TAG_MODE);
        let mut list: Vec<(u8, CardId)> = self
            .state
            .cards
            .values()
            .filter(|c| {
                let owner_match = if is_tag && location.intersects(Location::ONFIELD | Location::FZONE | Location::PZONE | Location::GRAVE | Location::REMOVED) {
                    self.team_of(c.controller) == team
                } else {
                    c.controller == controller
                };
                owner_match && c.location.intersects(location)
            })
            .map(|c| (c.sequence, c.id))
            .collect();
        list.sort_by_key(|&(seq, id)| (seq, id.0));
        list.into_iter().map(|(_, id)| id).collect()
    }

    pub fn card(&self, id: CardId) -> Option<&CardInstance> {
        self.state.cards.get(&id)
    }

    pub fn card_mut(&mut self, id: CardId) -> Option<&mut CardInstance> {
        self.state.cards.get_mut(&id)
    }

    pub fn card_at(&self, controller: PlayerId, location: Location, seq: u8) -> Option<CardId> {
        let team = self.team_of(controller);
        let is_tag = self.state.config.flags.contains(DuelFlags::TAG_MODE);
        // Handle shared EMZ lookup (seq 5 or 6)
        if location == Location::MZONE && (seq == 5 || seq == 6) {
            let target_grid = mzone_to_grid(controller, seq)?;
            return self
                .state
                .cards
                .values()
                .find(|c| {
                    c.location == Location::MZONE
                        && mzone_to_grid(c.controller, c.sequence) == Some(target_grid)
                })
                .map(|c| c.id);
        }
        self.state
            .cards
            .values()
            .find(|c| {
                let ctrl_ok = if is_tag {
                    self.team_of(c.controller) == team
                } else {
                    c.controller == controller
                };
                ctrl_ok && c.location == location && c.sequence == seq
            })
            .map(|c| c.id)
    }

    pub fn next_sequence_for(&self, controller: PlayerId, location: Location) -> u8 {
        match location {
            Location::MZONE => self
                .free_mmz_zones(controller)
                .into_iter()
                .next()
                .unwrap_or(0),
            Location::SZONE => self
                .free_stz_zones(controller)
                .into_iter()
                .next()
                .unwrap_or(0),
            Location::FZONE => 5,
            Location::PZONE => {
                let (l, r) = self.state.config.pendulum_zone_seqs();
                if self.card_at(controller, Location::PZONE, l).is_none()
                    && self.card_at(controller, Location::SZONE, l).is_none()
                {
                    l
                } else {
                    r
                }
            }
            _ => self.cards_in(controller, location).len() as u8,
        }
    }

    pub fn free_mmz_zones(&self, controller: PlayerId) -> Vec<u8> {
        let count = self.state.config.mmz_count() as u8;
        (0..count)
            .filter(|&seq| self.card_at(controller, Location::MZONE, seq).is_none())
            .collect()
    }

    pub fn free_stz_zones(&self, controller: PlayerId) -> Vec<u8> {
        let count = self.state.config.stz_count() as u8;
        (0..count)
            .filter(|&seq| {
                self.card_at(controller, Location::SZONE, seq).is_none()
                    && self.card_at(controller, Location::PZONE, seq).is_none()
            })
            .collect()
    }

    /// Collect all face-up Link Monsters `(controller, seq, markers)` currently on the field.
    pub fn active_link_monsters(&self) -> Vec<(PlayerId, u8, LinkMarker)> {
        self.state
            .cards
            .values()
            .filter(|c| {
                c.location == Location::MZONE
                    && c.is_faceup()
                    && c.data.is_link()
                    && !c.is_disabled()
            })
            .map(|c| (c.controller, c.sequence, c.data.link_markers))
            .collect()
    }

    /// Check if `player`'s Main Monster Zone `seq` (0..5) is currently pointed to by any Link Monster.
    pub fn is_mmz_linked(&self, player: PlayerId, seq: u8) -> bool {
        let Some(target) = mzone_to_grid(player, seq) else {
            return false;
        };
        for (ctrl, lseq, markers) in self.active_link_monsters() {
            if pointed_grid_positions(ctrl, lseq, markers).contains(&target) {
                return true;
            }
        }
        false
    }

    /// All cards currently pointed to by `card_id` (Link Arrows).
    pub fn linked_cards(&self, card_id: CardId) -> Vec<CardId> {
        let Some(c) = self.card(card_id) else {
            return Vec::new();
        };
        if c.location != Location::MZONE || !c.is_faceup() || !c.data.is_link() {
            return Vec::new();
        }
        let targets = pointed_grid_positions(c.controller, c.sequence, c.data.link_markers);
        self.state
            .cards
            .values()
            .filter(|other| {
                other.location == Location::MZONE
                    && mzone_to_grid(other.controller, other.sequence)
                        .map(|g| targets.contains(&g))
                        .unwrap_or(false)
            })
            .map(|other| other.id)
            .collect()
    }

    /// Check if two Link Monsters are mutually Co-Linked.
    pub fn are_cards_co_linked(&self, a: CardId, b: CardId) -> bool {
        let (Some(ca), Some(cb)) = (self.card(a), self.card(b)) else {
            return false;
        };
        if ca.location != Location::MZONE || cb.location != Location::MZONE {
            return false;
        }
        if !ca.is_faceup() || !cb.is_faceup() || !ca.data.is_link() || !cb.data.is_link() {
            return false;
        }
        are_co_linked(
            ca.controller,
            ca.sequence,
            ca.data.link_markers,
            cb.controller,
            cb.sequence,
            cb.data.link_markers,
        )
    }

    /// Check if `card_id` is part of an Extra Link (U-Link).
    pub fn is_card_extra_linked(&self, card_id: CardId) -> bool {
        let Some(c) = self.card(card_id) else {
            return false;
        };
        let Some(pos) = mzone_to_grid(c.controller, c.sequence) else {
            return false;
        };
        let links = self.active_link_monsters();
        extra_linked_positions(&links).contains(&pos)
    }

    /// Valid monster zones for Special Summoning from the Extra Deck under the active Master Rule!
    /// Includes EMZ eligibility AND Extra Link (U-Link) second-EMZ claim!
    pub fn valid_extra_deck_zones(&self, player: PlayerId, card_data: &CardData) -> Vec<u8> {
        let requires_emz_or_link = self
            .state
            .config
            .extra_deck_requires_emz_or_link(card_data.card_type);
        let mut zones = Vec::new();

        if !requires_emz_or_link {
            // MR1/2/3 or MR5 Fusion/Synchro/Xyz: any free MMZ is valid!
            zones.extend(self.free_mmz_zones(player));
        } else {
            // Any free Linked MMZ
            for seq in self.free_mmz_zones(player) {
                if self.is_mmz_linked(player, seq) {
                    zones.push(seq);
                }
            }
        }

        // Check EMZ 5 and 6 if EMZs exist
        if self.state.config.has_emz() {
            let team = self.team_of(player);
            let player_already_has_emz = [5u8, 6u8].iter().any(|&emz| {
                self.card_at(player, Location::MZONE, emz)
                    .and_then(|cid| self.card(cid))
                    .map(|c| self.team_of(c.controller) == team)
                    .unwrap_or(false)
            });

            let active_links = self.active_link_monsters();
            for emz in [5u8, 6u8] {
                if self.card_at(player, Location::MZONE, emz).is_none() {
                    if !player_already_has_emz {
                        zones.push(emz);
                    } else if card_data.is_link()
                        && would_complete_extra_link(&active_links, player, emz, card_data.link_markers)
                    {
                        // Official Extra Link (U-Link) rule: player MAY occupy the second EMZ if it completes a U-Link!
                        zones.push(emz);
                    }
                }
            }
        }

        zones.sort_unstable();
        zones.dedup();
        zones
    }

    /// Get all cards in the same column as `card_id` (Mekk-Knight / Infinite Impermanence column mechanics!).
    pub fn column_cards(&self, card_id: CardId) -> Vec<CardId> {
        let Some(c) = self.card(card_id) else {
            return Vec::new();
        };
        let col = if c.location == Location::MZONE {
            mzone_to_grid(c.controller, c.sequence).map(|g| g.col)
        } else if c.location == Location::SZONE || c.location == Location::PZONE {
            if c.sequence < 5 {
                Some(if c.controller % 2 == 0 {
                    c.sequence as i8
                } else {
                    4 - c.sequence as i8
                })
            } else {
                None
            }
        } else {
            None
        };
        let Some(col) = col else {
            return Vec::new();
        };

        self.state
            .cards
            .values()
            .filter(|other| {
                other.id != card_id
                    && match other.location {
                        Location::MZONE => {
                            mzone_to_grid(other.controller, other.sequence).map(|g| g.col) == Some(col)
                        }
                        Location::SZONE | Location::PZONE if other.sequence < 5 => {
                            let ocol = if other.controller % 2 == 0 {
                                other.sequence as i8
                            } else {
                                4 - other.sequence as i8
                            };
                            ocol == col
                        }
                        _ => false,
                    }
            })
            .map(|other| other.id)
            .collect()
    }

    /// Get a player's current Left and Right Pendulum Scales if both Pendulum Zones are occupied.
    pub fn pendulum_scales(&self, player: PlayerId) -> Option<(u32, u32)> {
        let (lseq, rseq) = self.state.config.pendulum_zone_seqs();
        let left_id = self
            .card_at(player, Location::PZONE, lseq)
            .or_else(|| self.card_at(player, Location::SZONE, lseq))?;
        let right_id = self
            .card_at(player, Location::PZONE, rseq)
            .or_else(|| self.card_at(player, Location::SZONE, rseq))?;
        let left = self.card(left_id)?;
        let right = self.card(right_id)?;
        if !left.data.is_pendulum() || !right.data.is_pendulum() || !left.is_faceup() || !right.is_faceup() {
            return None;
        }
        Some((left.computed.lscale, right.computed.rscale))
    }

    // -------------------------------------------------------------------------
    // DECK / DRAW / CARD MOVEMENT / REDIRECTION
    // -------------------------------------------------------------------------

    pub fn shuffle_deck(&mut self, player: PlayerId) {
        let mut deck_ids = self.cards_in(player, Location::DECK);
        self.state.rng.shuffle(&mut deck_ids);
        for (idx, cid) in deck_ids.into_iter().enumerate() {
            if let Some(c) = self.state.cards.get_mut(&cid) {
                c.sequence = idx as u8;
            }
        }
        self.emit(DuelMessage::ShuffleDeck { player });
    }

    pub fn draw_cards(
        &mut self,
        player: PlayerId,
        count: usize,
        reason: Reason,
        host: &mut dyn ScriptHost,
    ) -> Vec<CardId> {
        if count == 0 || self.is_finished() {
            return Vec::new();
        }
        let team = self.team_of(player);
        if !reason.contains(Reason::RULE) && (self.state.floodgates.cannot_draw[team] || self.state.floodgates.cannot_to_hand_from_deck[team]) {
            return Vec::new();
        }
        let mut deck = self.cards_in(player, Location::DECK);
        if deck.len() < count {
            // Deck Out!
            let winner = self.opponent_of(player);
            self.declare_win(Some(winner), WinReason::DeckOut);
            return Vec::new();
        }

        let mut drawn = Vec::with_capacity(count);
        for _ in 0..count {
            if let Some(cid) = deck.pop() {
                let hand_seq = self.cards_in(player, Location::HAND).len() as u8;
                self.move_card_raw(
                    cid,
                    player,
                    Location::HAND,
                    hand_seq,
                    Position::FACEUP_ATTACK,
                    reason | Reason::DRAW,
                    player,
                    None,
                    host,
                );
                drawn.push(cid);
            }
        }
        if !drawn.is_empty() {
            self.emit(DuelMessage::Draw {
                player,
                cards: drawn.clone(),
            });
            self.raise_event(
                DuelEvent::new(event_code::DRAW, player)
                    .with_cards(drawn.clone())
                    .with_value(drawn.len() as i64)
                    .with_reason(reason | Reason::DRAW, player, None),
            );
            self.check_win_conditions();
        }
        drawn
    }

    /// Move a card between zones obeying ALL Yu-Gi-Oh! redirection & cleanup rules:
    /// - Tokens leaving the field vanish completely
    /// - Pendulum monsters sent from Field to GY go face-up to Extra Deck instead!
    /// - Macro Cosmos / Dimensional Fissure redirects GY -> Banished!
    /// - Imperial Iron Wall prevents banishing (`cannot_remove`)!
    /// - Xyz Monsters leaving field send attached Overlay Materials to GY (without `LEAVE_FIELD`)!
    /// - Equip Spells detach cleanly when target leaves field!
    pub fn move_card(
        &mut self,
        card_id: CardId,
        to_controller: PlayerId,
        mut to_location: Location,
        to_sequence: Option<u8>,
        mut to_position: Position,
        mut reason: Reason,
        reason_player: PlayerId,
        reason_effect: Option<EffectId>,
        host: &mut dyn ScriptHost,
    ) -> Result<bool, DuelError> {
        let card = self
            .card(card_id)
            .ok_or(DuelError::CardNotFound(card_id))?
            .clone();

        // Imperial Iron Wall: cannot banish!
        if to_location == Location::REMOVED {
            let team = self.team_of(to_controller);
            if self.state.floodgates.cannot_remove[team] && !reason.contains(Reason::RULE) {
                return Ok(false);
            }
        }

        // Droll & Lock Bird: cannot add from Deck to Hand (except normal draw)
        if card.location == Location::DECK
            && to_location == Location::HAND
            && !reason.contains(Reason::RULE)
            && self.state.floodgates.cannot_to_hand_from_deck[self.team_of(to_controller)]
        {
            return Ok(false);
        }

        // Token leaving the field is removed from the game entirely
        if card.data.card_type.contains(CardType::TOKEN) && !to_location.intersects(Location::ONFIELD) {
            self.state.cards.remove(&card_id);
            self.emit(DuelMessage::Move {
                card_id,
                code: card.data.code,
                from_controller: card.controller,
                from_location: card.location,
                from_sequence: card.sequence,
                to_controller,
                to_location: Location::NONE,
                to_sequence: 0,
                to_position: Position::NONE,
                reason,
            });
            self.recalculate_effects(host);
            return Ok(true);
        }

        // Official Macro Cosmos / Dimensional Fissure redirection!
        if to_location == Location::GRAVE {
            let team = self.team_of(card.owner);
            let banish_all = self.state.floodgates.to_grave_redirect_to_banish[team];
            let banish_mon = self.state.floodgates.monsters_to_grave_redirect_to_banish[team]
                && card.data.is_monster();
            if (banish_all || banish_mon) && !self.state.floodgates.cannot_remove[team] {
                to_location = Location::REMOVED;
                to_position = Position::FACEUP_ATTACK;
                reason.insert(Reason::REDIRECT);
            }
        }

        // Official Pendulum Rule: Pendulum Monster sent from Field to GY goes face-up to Extra Deck!
        // (Unless it was Xyz material, negated summon, or redirected to Banished by Macro Cosmos)
        if to_location == Location::GRAVE
            && card.data.is_pendulum()
            && card.is_on_field()
            && !reason.contains(Reason::DISSUMMON)
        {
            to_location = Location::EXTRA;
            to_position = Position::FACEUP_ATTACK;
            reason.insert(Reason::REDIRECT);
        }

        // Extra Deck monsters going to Hand or Main Deck return face-down to Extra Deck instead!
        if (to_location == Location::HAND || to_location == Location::DECK) && card.data.is_extra_deck() {
            to_location = Location::EXTRA;
            to_position = Position::FACEDOWN_DEFENSE;
        }

        // Detach Xyz Overlay Materials to GY when an Xyz monster leaves the field
        // Official Rule: Overlay materials sent to GY do NOT count as "leaving the field"!
        let leaving_field = card.is_on_field() && !to_location.intersects(Location::ONFIELD | Location::FZONE | Location::PZONE);
        if leaving_field && !card.overlay_materials.is_empty() {
            let mats = card.overlay_materials.clone();
            if let Some(c) = self.card_mut(card_id) {
                c.overlay_materials.clear();
            }
            for mat_id in mats {
                if let Some(m) = self.card_mut(mat_id) {
                    m.overlay_parent = None;
                }
                let mat_owner = self.card(mat_id).map(|m| m.owner).unwrap_or(card.owner);
                let gy_seq = self.cards_in(mat_owner, Location::GRAVE).len() as u8;
                self.move_card_raw(
                    mat_id,
                    mat_owner,
                    Location::GRAVE,
                    gy_seq,
                    Position::FACEUP_ATTACK,
                    Reason::RULE,
                    reason_player,
                    None,
                    host,
                );
            }
        }

        // Clean up Equip relationships if leaving field
        if leaving_field {
            if let Some(eq_target) = card.equip_target {
                if let Some(t) = self.card_mut(eq_target) {
                    t.equipped_cards.retain(|&id| id != card_id);
                }
                if let Some(c) = self.card_mut(card_id) {
                    c.equip_target = None;
                }
            }
            let equips = card.equipped_cards.clone();
            if !equips.is_empty() {
                if let Some(c) = self.card_mut(card_id) {
                    c.equipped_cards.clear();
                }
                for eq_id in equips {
                    if let Some(eq) = self.card_mut(eq_id) {
                        eq.equip_target = None;
                    }
                    let eq_owner = self.card(eq_id).map(|e| e.owner).unwrap_or(card.owner);
                    let _ = self.move_card(
                        eq_id,
                        eq_owner,
                        Location::GRAVE,
                        None,
                        Position::FACEUP_ATTACK,
                        Reason::LOST_TARGET | Reason::DESTROY,
                        reason_player,
                        None,
                        host,
                    );
                }
            }
        }

        let final_ctrl = if matches!(
            to_location,
            Location::DECK | Location::HAND | Location::GRAVE | Location::REMOVED | Location::EXTRA
        ) {
            card.owner
        } else {
            to_controller
        };
        let final_seq = to_sequence.unwrap_or_else(|| self.next_sequence_for(final_ctrl, to_location));

        self.move_card_raw(
            card_id,
            final_ctrl,
            to_location,
            final_seq,
            to_position,
            reason,
            reason_player,
            reason_effect,
            host,
        );

        // Emit standard ocgcore movement events
        if leaving_field {
            self.raise_event(
                DuelEvent::new(event_code::LEAVE_FIELD, card.controller)
                    .with_cards(vec![card_id])
                    .with_reason(reason, reason_player, reason_effect),
            );
        }
        let loc_event = match to_location {
            Location::GRAVE => Some(event_code::TO_GRAVE),
            Location::HAND => Some(event_code::TO_HAND),
            Location::DECK | Location::EXTRA => Some(event_code::TO_DECK),
            Location::REMOVED => Some(event_code::REMOVE),
            _ => None,
        };
        if let Some(ev) = loc_event {
            self.raise_event(
                DuelEvent::new(ev, final_ctrl)
                    .with_cards(vec![card_id])
                    .with_reason(reason, reason_player, reason_effect),
            );
        }
        if reason.contains(Reason::DESTROY) {
            self.raise_event(
                DuelEvent::new(event_code::DESTROYED, final_ctrl)
                    .with_cards(vec![card_id])
                    .with_reason(reason, reason_player, reason_effect),
            );
        }

        self.recalculate_effects(host);
        self.check_win_conditions();
        Ok(true)
    }

    fn move_card_raw(
        &mut self,
        card_id: CardId,
        to_controller: PlayerId,
        to_location: Location,
        to_sequence: u8,
        to_position: Position,
        reason: Reason,
        reason_player: PlayerId,
        reason_effect: Option<EffectId>,
        _host: &mut dyn ScriptHost,
    ) {
        let Some(c) = self.state.cards.get_mut(&card_id) else {
            return;
        };
        let from_ctrl = c.controller;
        let from_loc = c.location;
        let from_seq = c.sequence;
        let code = c.data.code;

        c.prev_controller = c.controller;
        c.prev_location = c.location;
        c.prev_sequence = c.sequence;
        c.prev_position = c.position;

        c.controller = to_controller;
        c.location = to_location;
        c.sequence = to_sequence;
        c.position = to_position;
        c.reason = reason;
        c.reason_player = reason_player;
        c.reason_effect = reason_effect;

        // Reset field-only transient status & counters when leaving the field
        let on_field = to_location.intersects(Location::ONFIELD | Location::FZONE | Location::PZONE);
        if !on_field {
            c.counters.clear();
            c.attacks_declared_this_turn = 0;
            c.manual_atk_override = None;
            c.manual_def_override = None;
            c.status.remove(
                CardStatus::SUMMON_TURN
                    | CardStatus::SPSUMMON_TURN
                    | CardStatus::FLIP_SUMMON_TURN
                    | CardStatus::SET_TURN
                    | CardStatus::FORM_CHANGED
                    | CardStatus::BATTLE_DESTROYED
                    | CardStatus::DISABLED
                    | CardStatus::EFFECT_ENABLED
                    | CardStatus::GEMINI_SUMMONED
                    | CardStatus::MAXIMUM_MODE,
            );
            // If returned to Deck or face-down Extra Deck, reset PROC_COMPLETE!
            if to_location == Location::DECK
                || (to_location == Location::EXTRA && to_position.intersects(Position::FACEDOWN))
            {
                c.status.remove(CardStatus::PROC_COMPLETE);
            }
            // Expire RESET_LEAVE / RESET_TOGRAVE / RESET_REMOVE effects on this card
            let to_remove: Vec<EffectId> = c.effects.clone();
            for eid in to_remove {
                if let Some(eff) = self.state.effects.get_mut(&eid) {
                    if eff.reset_flags.intersects(
                        ResetFlag::LEAVE
                            | ResetFlag::TOGRAVE
                            | ResetFlag::REMOVE
                            | ResetFlag::TOHAND
                            | ResetFlag::TODECK,
                    ) {
                        eff.active = false;
                    }
                }
            }
        } else if to_position.intersects(Position::FACEUP) {
            c.status.insert(CardStatus::EFFECT_ENABLED);
        }

        self.emit(DuelMessage::Move {
            card_id,
            code,
            from_controller: from_ctrl,
            from_location: from_loc,
            from_sequence: from_seq,
            to_controller,
            to_location,
            to_sequence,
            to_position,
            reason,
        });
    }

    /// Destroy one or more cards (respecting Indestructible by Effect & Immunity).
    pub fn destroy_cards(
        &mut self,
        targets: &[CardId],
        reason: Reason,
        reason_player: PlayerId,
        reason_effect: Option<EffectId>,
        host: &mut dyn ScriptHost,
    ) -> Vec<CardId> {
        let mut destroyed = Vec::new();
        for &cid in targets {
            let Some(c) = self.card(cid).cloned() else {
                continue;
            };
            if reason.contains(Reason::EFFECT) {
                if c.computed.indestructable_effect || self.is_card_immune_to_effect(cid, reason_effect) {
                    continue;
                }
            }
            let owner = c.owner;
            if self
                .move_card(
                    cid,
                    owner,
                    Location::GRAVE,
                    None,
                    Position::FACEUP_ATTACK,
                    reason | Reason::DESTROY,
                    reason_player,
                    reason_effect,
                    host,
                )
                .unwrap_or(false)
            {
                destroyed.push(cid);
            }
        }
        destroyed
    }

    pub fn is_card_immune_to_effect(&self, card_id: CardId, effect_id: Option<EffectId>) -> bool {
        let Some(c) = self.card(card_id) else {
            return false;
        };
        if c.computed.immune_to_all_effects {
            return true;
        }
        let Some(eid) = effect_id else {
            return false;
        };
        let Some(eff) = self.state.effects.get(&eid) else {
            return false;
        };
        if eff.flags.contains(EffectFlag::IGNORE_IMMUNE) {
            return false;
        }
        let Some(handler_id) = eff.handler_card else {
            return false;
        };
        let Some(handler) = self.card(handler_id) else {
            return false;
        };
        if c.computed.immune_to_spells && handler.data.is_spell() {
            return true;
        }
        if c.computed.immune_to_traps && handler.data.is_trap() {
            return true;
        }
        if c.computed.immune_to_monsters && handler.data.is_monster() {
            return true;
        }
        false
    }

    // -------------------------------------------------------------------------
    // CONTINUOUS EFFECT & FLOODGATE RECALCULATION (TIMESTAMP ORDERED)
    // -------------------------------------------------------------------------

    pub fn recalculate_effects(&mut self, host: &mut dyn ScriptHost) {
        // 1. Reset computed stats on all cards to base values
        for card in self.state.cards.values_mut() {
            card.computed = CardInstance::base_computed(&card.data);
            if card.status.contains(CardStatus::DISABLED) {
                card.computed.negated = true;
            }
        }
        self.state.floodgates = GlobalFloodgates::default();

        // 2. Gather active continuous / single / field / equip effects in timestamp order
        let mut active_effects: Vec<Effect> = self
            .state
            .effects
            .values()
            .filter(|e| {
                e.active
                    && e.effect_type.intersects(
                        EffectType::SINGLE
                            | EffectType::FIELD
                            | EffectType::EQUIP
                            | EffectType::CONTINUOUS
                            | EffectType::UNCLASSIFIED,
                    )
            })
            .cloned()
            .collect();
        active_effects.sort_by_key(|e| e.creation_order);

        // Pass A: Apply `EFFECT_DISABLE` (Skill Drain / Veiler / Impermanence) first!
        for eff in &active_effects {
            if eff.code != effect_code::DISABLE && eff.code != effect_code::DISABLE_EFFECT {
                continue;
            }
            if !self.is_continuous_effect_in_range(eff) {
                continue;
            }
            if !self.check_effect_condition(eff, eff.owner_player, None, host) {
                continue;
            }
            for target_id in self.continuous_effect_targets(eff) {
                if let Some(c) = self.state.cards.get_mut(&target_id) {
                    c.computed.negated = true;
                }
            }
        }

        // Pass B: Apply all other continuous modifiers & floodgates (skipping effects of negated cards!)
        for eff in &active_effects {
            if eff.code == effect_code::DISABLE || eff.code == effect_code::DISABLE_EFFECT {
                continue;
            }
            if !self.is_continuous_effect_in_range(eff) {
                continue;
            }
            // If handler is negated on field and effect lacks CANNOT_DISABLE, skip!
            if let Some(hid) = eff.handler_card {
                if let Some(h) = self.state.cards.get(&hid) {
                    if h.is_disabled() && !eff.flags.contains(EffectFlag::CANNOT_DISABLE) {
                        continue;
                    }
                }
            }
            if !self.check_effect_condition(eff, eff.owner_player, None, host) {
                continue;
            }

            // Player-target floodgates (`EFFECT_FLAG_PLAYER_TARGET`)
            if eff.flags.contains(EffectFlag::PLAYER_TARGET) {
                let owner_team = self.team_of(eff.owner_player);
                let opp_team = 1 - owner_team;
                let mut affected_teams = Vec::new();
                if eff.target_range.0 != 0 {
                    affected_teams.push(owner_team);
                }
                if eff.target_range.1 != 0 {
                    affected_teams.push(opp_team);
                }
                for team in affected_teams {
                    match eff.code {
                        effect_code::CANNOT_SPECIAL_SUMMON => {
                            self.state.floodgates.cannot_special_summon[team] = true;
                        }
                        effect_code::CANNOT_DRAW => {
                            self.state.floodgates.cannot_draw[team] = true;
                        }
                        effect_code::CANNOT_TO_HAND => {
                            self.state.floodgates.cannot_to_hand_from_deck[team] = true;
                        }
                        effect_code::CANNOT_REMOVE => {
                            self.state.floodgates.cannot_remove[team] = true;
                        }
                        effect_code::CANNOT_ACTIVATE => {
                            let v = self.eval_effect_value(eff, None, host);
                            if v == 0 || (v as u32 & CardType::MONSTER.bits()) != 0 {
                                self.state.floodgates.cannot_activate_monster_effects[team] = true;
                            }
                            if v == 0 || (v as u32 & CardType::SPELL.bits()) != 0 {
                                self.state.floodgates.cannot_activate_spells[team] = true;
                            }
                            if v == 0 || (v as u32 & CardType::TRAP.bits()) != 0 {
                                self.state.floodgates.cannot_activate_traps[team] = true;
                            }
                        }
                        effect_code::EXTRA_SUMMON_COUNT => {
                            let val = self.eval_effect_value(eff, None, host).max(1) as u32;
                            self.state.floodgates.extra_normal_summons[team] += val;
                        }
                        effect_code::HAND_LIMIT => {
                            let val = self.eval_effect_value(eff, None, host).max(0) as usize;
                            self.state.floodgates.hand_limit_override[team] = Some(val);
                        }
                        _ => {}
                    }
                }
                continue;
            }

            // Global GY redirect floodgates (Macro Cosmos / Dimensional Fissure)
            if eff.code == effect_code::TO_GRAVE_REDIRECT && eff.effect_type.contains(EffectType::FIELD) {
                let val = self.eval_effect_value(eff, None, host) as u32;
                if val == Location::REMOVED.bits() {
                    let owner_team = self.team_of(eff.owner_player);
                    let opp_team = 1 - owner_team;
                    let monsters_only = eff.label == 1;
                    for (range_bits, team) in [(eff.target_range.0, owner_team), (eff.target_range.1, opp_team)] {
                        if range_bits != 0 {
                            if monsters_only {
                                self.state.floodgates.monsters_to_grave_redirect_to_banish[team] = true;
                            } else {
                                self.state.floodgates.to_grave_redirect_to_banish[team] = true;
                            }
                        }
                    }
                }
                continue;
            }

            // Card-targeting continuous modifiers
            let targets = self.continuous_effect_targets(eff);
            for tid in targets {
                let val = self.eval_effect_value(eff, Some(tid), host);
                let Some(card) = self.state.cards.get_mut(&tid) else {
                    continue;
                };
                match eff.code {
                    effect_code::UPDATE_ATTACK => card.computed.atk = (card.computed.atk + val as i32).max(0),
                    effect_code::SET_ATTACK | effect_code::SET_ATTACK_FINAL => card.computed.atk = (val as i32).max(0),
                    effect_code::SET_BASE_ATTACK | effect_code::SET_BASE_ATTACK_FINAL => {
                        card.computed.base_atk = (val as i32).max(0);
                        card.computed.atk = card.computed.base_atk;
                    }
                    effect_code::UPDATE_DEFENSE => card.computed.def = (card.computed.def + val as i32).max(0),
                    effect_code::SET_DEFENSE | effect_code::SET_DEFENSE_FINAL => card.computed.def = (val as i32).max(0),
                    effect_code::SET_BASE_DEFENSE | effect_code::SET_BASE_DEFENSE_FINAL => {
                        card.computed.base_def = (val as i32).max(0);
                        card.computed.def = card.computed.base_def;
                    }
                    effect_code::SWAP_AD => {
                        std::mem::swap(&mut card.computed.atk, &mut card.computed.def);
                    }
                    effect_code::UPDATE_LEVEL => {
                        card.computed.level = (card.computed.level as i64 + val).max(1) as u32;
                    }
                    effect_code::CHANGE_LEVEL => {
                        card.computed.level = val.max(1) as u32;
                    }
                    effect_code::UPDATE_LSCALE => {
                        card.computed.lscale = (card.computed.lscale as i64 + val).max(0) as u32;
                    }
                    effect_code::UPDATE_RSCALE => {
                        card.computed.rscale = (card.computed.rscale as i64 + val).max(0) as u32;
                    }
                    effect_code::CHANGE_RACE => {
                        card.computed.race = Race::from_bits_truncate(val as u64);
                    }
                    effect_code::ADD_RACE => {
                        card.computed.race.insert(Race::from_bits_truncate(val as u64));
                    }
                    effect_code::CHANGE_ATTRIBUTE => {
                        card.computed.attribute = Attribute::from_bits_truncate(val as u32);
                    }
                    effect_code::ADD_ATTRIBUTE => {
                        card.computed.attribute.insert(Attribute::from_bits_truncate(val as u32));
                    }
                    effect_code::CHANGE_CODE => {
                        card.computed.code = val as CardCode;
                    }
                    effect_code::ADD_CODE => {
                        let code = val as CardCode;
                        if !card.computed.extra_codes.contains(&code) {
                            card.computed.extra_codes.push(code);
                        }
                    }
                    effect_code::INDESTRUCTABLE_BATTLE => card.computed.indestructable_battle = true,
                    effect_code::INDESTRUCTABLE_EFFECT => card.computed.indestructable_effect = true,
                    effect_code::INDESTRUCTABLE => {
                        card.computed.indestructable_battle = true;
                        card.computed.indestructable_effect = true;
                    }
                    effect_code::CANNOT_BE_EFFECT_TARGET => card.computed.untargetable_effect = true,
                    effect_code::CANNOT_BE_BATTLE_TARGET => card.computed.untargetable_battle = true,
                    effect_code::CANNOT_ATTACK | effect_code::CANNOT_ATTACK_ANNOUNCE => {
                        card.computed.cannot_attack = true;
                    }
                    effect_code::DIRECT_ATTACK => card.computed.direct_attack = true,
                    effect_code::PIERCE => card.computed.pierce = true,
                    effect_code::EXTRA_ATTACK => card.computed.extra_attacks += val.max(1) as u32,
                    effect_code::IMMUNE_EFFECT => match val {
                        0 | 1 => card.computed.immune_to_all_effects = true,
                        2 => card.computed.immune_to_spells = true,
                        4 => card.computed.immune_to_traps = true,
                        8 => card.computed.immune_to_monsters = true,
                        _ => card.computed.immune_to_all_effects = true,
                    },
                    _ => {}
                }
            }
        }

        // Update normal summon limits from floodgates/effects
        for p in &mut self.state.players {
            let team = (p.id % 2) as usize;
            if self.state.config.is_rush() {
                p.normal_summons_limit = u32::MAX;
            } else {
                p.normal_summons_limit = 1 + self.state.floodgates.extra_normal_summons[team];
            }
        }

        // Official Equip Spell Upkeep Rule: if an equipped monster flips face-down or leaves MZONE,
        // destroy the Equip Spell with REASON_LOST_TARGET | REASON_DESTROY!
        let orphaned_equips: Vec<(CardId, CardId, PlayerId)> = self
            .state
            .cards
            .values()
            .filter_map(|eq| {
                let tid = eq.equip_target?;
                if !eq.is_on_field() {
                    return None;
                }
                let target_ok = self
                    .state
                    .cards
                    .get(&tid)
                    .map(|t| t.location == Location::MZONE && t.is_faceup())
                    .unwrap_or(false);
                if !target_ok {
                    Some((eq.id, tid, eq.owner))
                } else {
                    None
                }
            })
            .collect();
        for (eq_id, tid, owner) in orphaned_equips {
            if let Some(eq) = self.state.cards.get_mut(&eq_id) {
                eq.equip_target = None;
            }
            if let Some(t) = self.state.cards.get_mut(&tid) {
                t.equipped_cards.retain(|&id| id != eq_id);
            }
            let _ = self.move_card(
                eq_id,
                owner,
                Location::GRAVE,
                None,
                Position::FACEUP_ATTACK,
                Reason::LOST_TARGET | Reason::DESTROY,
                owner,
                None,
                host,
            );
        }
    }

    fn is_continuous_effect_in_range(&self, eff: &Effect) -> bool {
        if eff.is_player_effect {
            return true;
        }
        let Some(hid) = eff.handler_card else {
            return true;
        };
        let Some(handler) = self.state.cards.get(&hid) else {
            return false;
        };
        if eff.effect_type.contains(EffectType::EQUIP) {
            return handler.is_on_field() && handler.is_faceup() && handler.equip_target.is_some();
        }
        if eff.range.is_empty() {
            // Single temporary effect registered directly on a card (e.g. from a Spell resolution)
            return handler.is_on_field() && handler.is_faceup();
        }
        handler.location.intersects(eff.range)
            && (!handler.is_on_field() || handler.is_faceup() || eff.flags.contains(EffectFlag::SET_AVAILABLE))
    }

    fn continuous_effect_targets(&self, eff: &Effect) -> Vec<CardId> {
        if eff.effect_type.contains(EffectType::EQUIP) {
            return eff
                .handler_card
                .and_then(|hid| self.state.cards.get(&hid))
                .and_then(|h| h.equip_target)
                .into_iter()
                .collect();
        }
        if eff.effect_type.contains(EffectType::SINGLE) {
            return eff.handler_card.into_iter().collect();
        }
        if eff.effect_type.contains(EffectType::FIELD) {
            let owner_team = self.team_of(eff.owner_player);
            let self_loc = Location::from_bits_truncate(eff.target_range.0);
            let opp_loc = Location::from_bits_truncate(eff.target_range.1);
            return self
                .state
                .cards
                .values()
                .filter(|c| {
                    if c.is_on_field() && !c.is_faceup() && !eff.flags.contains(EffectFlag::SET_AVAILABLE) {
                        return false;
                    }
                    let same_side = self.team_of(c.controller) == owner_team;
                    if same_side {
                        c.location.intersects(self_loc)
                    } else {
                        c.location.intersects(opp_loc)
                    }
                })
                .map(|c| c.id)
                .collect();
        }
        Vec::new()
    }

    fn eval_effect_value(&mut self, eff: &Effect, target: Option<CardId>, host: &mut dyn ScriptHost) -> i64 {
        match &eff.value {
            EffectValue::None => 0,
            EffectValue::Int(v) => *v,
            EffectValue::Callback(EffectCallback::LuaFunc(_)) => host.eval_value(self, eff.id, target),
            EffectValue::Callback(EffectCallback::Builtin(name)) => {
                self.eval_builtin_value(name, eff.id, target)
            }
            EffectValue::Callback(EffectCallback::None) => 0,
        }
    }

    fn eval_builtin_value(&self, _name: &str, _effect_id: EffectId, _target: Option<CardId>) -> i64 {
        0
    }

    fn check_effect_condition(
        &mut self,
        eff: &Effect,
        tp: PlayerId,
        event: Option<&DuelEvent>,
        host: &mut dyn ScriptHost,
    ) -> bool {
        match &eff.condition {
            EffectCallback::None => true,
            EffectCallback::LuaFunc(_) => host.eval_condition(self, eff.id, tp, event),
            EffectCallback::Builtin(name) => self.eval_builtin_condition(name, eff.id, tp, event),
        }
    }

    fn eval_builtin_condition(
        &self,
        name: &str,
        _effect_id: EffectId,
        tp: PlayerId,
        _event: Option<&DuelEvent>,
    ) -> bool {
        match name {
            "turn_player_only" => self.state.turn_player == tp,
            "main_phase_only" => self.state.phase.is_main_phase(),
            "battle_phase_only" => self.state.phase.is_battle_phase(),
            "no_monsters_controlled" => self.cards_in(tp, Location::MZONE).is_empty(),
            _ => true,
        }
    }

    // -------------------------------------------------------------------------
    // TURN & PHASE PROGRESSION
    // -------------------------------------------------------------------------

    pub fn begin_turn(&mut self, player: PlayerId, host: &mut dyn ScriptHost) {
        if self.is_finished() {
            return;
        }
        self.state.turn_count += 1;
        self.state.turn_player = player;
        self.state.entered_battle_phase = false;
        self.state.phase = Phase::Draw;
        self.state.battle = BattleState::default();

        // Reset per-turn player counters
        for p in &mut self.state.players {
            p.normal_summons_used = 0;
            p.pendulum_summoned_this_turn = false;
            p.opt_used_this_turn.clear();
            p.summons_this_turn = 0;
        }
        // Reset per-turn card status & attack counters
        for c in self.state.cards.values_mut() {
            c.attacks_declared_this_turn = 0;
            c.status.remove(
                CardStatus::SUMMON_TURN
                    | CardStatus::SPSUMMON_TURN
                    | CardStatus::FLIP_SUMMON_TURN
                    | CardStatus::SET_TURN
                    | CardStatus::FORM_CHANGED
                    | CardStatus::ATTACK_CANCELED,
            );
        }
        // Reset Soft-OPT per-turn counters
        for eff in self.state.effects.values_mut() {
            if let Some(cl) = &mut eff.count_limit {
                cl.used_this_turn = 0;
            }
        }

        self.emit(DuelMessage::NewTurn {
            turn: self.state.turn_count,
            player,
        });

        // Draw Phase
        self.emit(DuelMessage::NewPhase { phase: Phase::Draw });
        let skip_t1_draw = self.state.turn_count == 1
            && !self.state.config.flags.contains(DuelFlags::TURN1_DRAW);
        if !skip_t1_draw {
            let draw_n = if self.state.config.is_rush() {
                // Rush Duel rule: draw until you have 5 cards in hand (or draw 1 if you already have >=5)
                let hand_len = self.cards_in(player, Location::HAND).len();
                if hand_len < 5 { 5 - hand_len } else { 1 }
            } else {
                self.state.config.draw_per_turn
            };
            self.draw_cards(player, draw_n, Reason::RULE, host);
        }
        if self.is_finished() {
            return;
        }

        // Standby Phase
        self.advance_phase(Phase::Standby, host);
        if self.is_finished() {
            return;
        }
        // Turbo Duel: both players gain +1 Speed Counter (max 12) each Standby Phase
        if self.state.config.flags.contains(DuelFlags::TURBO_DUEL) {
            for p_idx in 0..2 {
                let sc = &mut self.state.players[p_idx].speed_counters;
                if *sc < 12 {
                    *sc += 1;
                }
                let val = *sc;
                self.emit(DuelMessage::SpeedCountersUpdated {
                    player: p_idx as PlayerId,
                    counters: val,
                });
            }
        }

        // Main Phase 1
        self.advance_phase(Phase::Main1, host);
    }

    pub fn advance_phase(&mut self, next_phase: Phase, host: &mut dyn ScriptHost) {
        if self.is_finished() {
            return;
        }
        self.state.phase = next_phase;
        if next_phase == Phase::BattleStart {
            self.state.entered_battle_phase = true;
        }
        self.emit(DuelMessage::NewPhase { phase: next_phase });
        self.raise_event(DuelEvent::new(
            event_code::PHASE_START + next_phase.code(),
            self.state.turn_player,
        ));
        self.expire_phase_effects(next_phase, host);
        self.recalculate_effects(host);
    }

    pub fn end_turn(&mut self, host: &mut dyn ScriptHost) {
        if self.is_finished() {
            return;
        }
        let tp = self.state.turn_player;
        self.advance_phase(Phase::End, host);

        // Spirit Monsters return to owner's hand during End Phase if summoned/flipped this turn
        let spirits: Vec<CardId> = self
            .state
            .cards
            .values()
            .filter(|c| {
                c.location == Location::MZONE
                    && c.is_faceup()
                    && c.data.card_type.contains(CardType::SPIRIT)
                    && c.status.intersects(CardStatus::SUMMON_TURN | CardStatus::FLIP_SUMMON_TURN)
                    && !c.is_disabled()
            })
            .map(|c| c.id)
            .collect();
        for sid in spirits {
            let owner = self.card(sid).map(|c| c.owner).unwrap_or(tp);
            let _ = self.move_card(
                sid,
                owner,
                Location::HAND,
                None,
                Position::FACEUP_ATTACK,
                Reason::EFFECT | Reason::RETURN,
                tp,
                None,
                host,
            );
        }

        // End Phase Hand Size Limit discard (default 6, unlimited in Rush Duel)
        let team = self.team_of(tp);
        let limit = self.state.floodgates.hand_limit_override[team]
            .unwrap_or(self.state.config.hand_size_limit);
        let hand = self.cards_in(tp, Location::HAND);
        if hand.len() > limit {
            let excess = hand.len() - limit;
            for &cid in hand.iter().rev().take(excess) {
                let _ = self.move_card(
                    cid,
                    tp,
                    Location::GRAVE,
                    None,
                    Position::FACEUP_ATTACK,
                    Reason::RULE | Reason::DISCARD,
                    tp,
                    None,
                    host,
                );
            }
        }

        self.expire_phase_effects(Phase::End, host);
        self.recalculate_effects(host);

        if self.is_finished() {
            return;
        }

        // Determine next turn player (handles Standard 2P and Tag Duel 4P `0 -> 2 -> 1 -> 3`)
        let next_player = if self.state.config.flags.contains(DuelFlags::TAG_MODE) {
            const TAG_ORDER: [PlayerId; 4] = [0, 1, 2, 3];
            self.state.tag_turn_slot = (self.state.tag_turn_slot + 1) % 4;
            let np = TAG_ORDER[self.state.tag_turn_slot];
            self.state.active_seat[self.team_of(np)] = np;
            np
        } else {
            1 - (tp % 2)
        };

        self.begin_turn(next_player, host);
    }

    fn expire_phase_effects(&mut self, current_phase: Phase, _host: &mut dyn ScriptHost) {
        let tp = self.state.turn_player;
        for eff in self.state.effects.values_mut() {
            if !eff.active || !eff.reset_flags.contains(ResetFlag::PHASE) {
                continue;
            }
            if let Some(target_phase) = eff.reset_phase {
                if target_phase != current_phase {
                    continue;
                }
            } else if current_phase != Phase::End {
                continue;
            }
            let side_ok = if eff.reset_flags.contains(ResetFlag::SELF_TURN) {
                (eff.owner_player % 2) == (tp % 2)
            } else if eff.reset_flags.contains(ResetFlag::OPPO_TURN) {
                (eff.owner_player % 2) != (tp % 2)
            } else {
                true
            };
            if side_ok {
                if eff.reset_count <= 1 {
                    eff.active = false;
                } else {
                    eff.reset_count -= 1;
                }
            }
        }
    }

    // -------------------------------------------------------------------------
    // SUMMONING MECHANICS (NORMAL, TRIBUTE, FLIP, RITUAL, FUSION, SYNCHRO, XYZ, PENDULUM, LINK, MAXIMUM)
    // -------------------------------------------------------------------------

    pub fn can_normal_summon(&self, player: PlayerId, card_id: CardId) -> bool {
        if self.state.turn_player != player || !self.state.phase.is_main_phase() || !self.state.chain.is_empty() {
            return false;
        }
        let p = &self.state.players[player as usize];
        if p.normal_summons_used >= p.normal_summons_limit {
            return false;
        }
        let Some(c) = self.card(card_id) else {
            return false;
        };
        if !c.data.is_monster() || c.data.is_extra_deck() || c.data.is_ritual() || c.data.card_type.contains(CardType::SPSUMMON) {
            return false;
        }
        // Gemini second Normal Summon on field!
        if c.location == Location::MZONE
            && c.controller == player
            && c.is_faceup()
            && c.data.card_type.contains(CardType::GEMINI)
            && !c.status.contains(CardStatus::GEMINI_SUMMONED)
        {
            return true;
        }
        if c.location != Location::HAND || c.controller != player {
            return false;
        }
        let req_tributes = c.data.required_tributes() as usize;
        let my_monsters = self.cards_in(player, Location::MZONE).len();
        if req_tributes == 0 {
            !self.free_mmz_zones(player).is_empty()
        } else {
            my_monsters >= req_tributes
        }
    }

    /// Perform a Normal Summon or Tribute Summon (or Normal Set if `face_down == true`).
    pub fn normal_summon(
        &mut self,
        player: PlayerId,
        card_id: CardId,
        tributes: &[CardId],
        zone: Option<u8>,
        face_down: bool,
        host: &mut dyn ScriptHost,
    ) -> Result<(), DuelError> {
        if !self.can_normal_summon(player, card_id) {
            return Err(DuelError::IllegalAction("cannot normal summon this card now".into()));
        }
        let card = self.card(card_id).ok_or(DuelError::CardNotFound(card_id))?.clone();

        // Gemini second Normal Summon on field
        if card.location == Location::MZONE && card.data.card_type.contains(CardType::GEMINI) {
            if let Some(c) = self.card_mut(card_id) {
                c.status.insert(CardStatus::GEMINI_SUMMONED | CardStatus::SUMMON_TURN);
                c.summon_type = SummonType::NORMAL | SummonType::GEMINI;
            }
            self.state.players[player as usize].normal_summons_used += 1;
            self.emit(DuelMessage::Summoned {
                card_id,
                summon_type: SummonType::NORMAL | SummonType::GEMINI,
            });
            self.recalculate_effects(host);
            return Ok(());
        }

        let req_tributes = card.data.required_tributes() as usize;
        if tributes.len() != req_tributes {
            return Err(DuelError::IllegalAction(format!(
                "expected {req_tributes} tributes, got {}",
                tributes.len()
            )));
        }
        for &tid in tributes {
            let t = self.card(tid).ok_or(DuelError::CardNotFound(tid))?;
            if t.location != Location::MZONE || self.team_of(t.controller) != self.team_of(player) {
                return Err(DuelError::IllegalAction("tribute must be a monster you control".into()));
            }
        }
        for &tid in tributes {
            let owner = self.card(tid).unwrap().owner;
            self.move_card(
                tid,
                owner,
                Location::GRAVE,
                None,
                Position::FACEUP_ATTACK,
                Reason::RELEASE | Reason::SUMMON,
                player,
                None,
                host,
            )?;
        }

        let target_seq = match zone {
            Some(z) if z < self.state.config.mmz_count() as u8 && self.card_at(player, Location::MZONE, z).is_none() => z,
            Some(z) => return Err(DuelError::InvalidZone(z)),
            None => *self
                .free_mmz_zones(player)
                .first()
                .ok_or_else(|| DuelError::IllegalAction("no free MMZ".into()))?,
        };

        let pos = if face_down {
            Position::FACEDOWN_DEFENSE
        } else {
            Position::FACEUP_ATTACK
        };
        let stype = if req_tributes > 0 {
            SummonType::NORMAL | SummonType::ADVANCE
        } else {
            SummonType::NORMAL
        };

        self.move_card(
            card_id,
            player,
            Location::MZONE,
            Some(target_seq),
            pos,
            Reason::SUMMON,
            player,
            None,
            host,
        )?;
        let turn_id = self.state.turn_count;
        if let Some(c) = self.card_mut(card_id) {
            c.summon_type = stype;
            c.summon_player = player;
            c.summon_location = Location::HAND;
            c.turn_id = turn_id;
            if face_down {
                c.status.insert(CardStatus::SET_TURN);
            } else {
                c.status.insert(CardStatus::SUMMON_TURN | CardStatus::EFFECT_ENABLED);
            }
        }
        self.state.players[player as usize].normal_summons_used += 1;
        self.state.players[player as usize].summons_this_turn += 1;

        if face_down {
            self.emit(DuelMessage::Set {
                card_id,
                controller: player,
                location: Location::MZONE,
                sequence: target_seq,
                position: pos,
            });
            self.raise_event(DuelEvent::new(event_code::MSET, player).with_cards(vec![card_id]));
        } else {
            self.emit(DuelMessage::Summoning {
                card_id,
                code: card.data.code,
                controller: player,
                sequence: target_seq,
                position: pos,
                summon_type: stype,
            });
            self.emit(DuelMessage::Summoned {
                card_id,
                summon_type: stype,
            });
            self.raise_event(
                DuelEvent::new(event_code::SUMMON_SUCCESS, player)
                    .with_cards(vec![card_id])
                    .with_reason(Reason::SUMMON, player, None),
            );
        }
        self.recalculate_effects(host);
        Ok(())
    }

    /// Flip Summon or change Battle Position of a monster you control (once per turn in MP1/MP2).
    pub fn change_position_manual(
        &mut self,
        player: PlayerId,
        card_id: CardId,
        host: &mut dyn ScriptHost,
    ) -> Result<(), DuelError> {
        if self.state.turn_player != player || !self.state.phase.is_main_phase() {
            return Err(DuelError::IllegalAction("can only change position in your Main Phase".into()));
        }
        let c = self.card(card_id).ok_or(DuelError::CardNotFound(card_id))?.clone();
        if c.location != Location::MZONE || self.team_of(c.controller) != self.team_of(player) {
            return Err(DuelError::IllegalAction("must control monster in MZONE".into()));
        }
        if c.data.is_link() {
            return Err(DuelError::IllegalAction("Link Monsters cannot change battle position".into()));
        }
        if c.status.intersects(
            CardStatus::SUMMON_TURN
                | CardStatus::SPSUMMON_TURN
                | CardStatus::SET_TURN
                | CardStatus::FLIP_SUMMON_TURN
                | CardStatus::FORM_CHANGED,
        ) || c.attacks_declared_this_turn > 0
        {
            return Err(DuelError::IllegalAction(
                "cannot change position this turn (summoned, set, attacked, or already changed)".into(),
            ));
        }

        let prev = c.position;
        let (new_pos, is_flip) = match prev {
            Position::FACEDOWN_DEFENSE | Position::FACEDOWN_ATTACK => (Position::FACEUP_ATTACK, true),
            Position::FACEUP_ATTACK => (Position::FACEUP_DEFENSE, false),
            Position::FACEUP_DEFENSE => (Position::FACEUP_ATTACK, false),
            _ => (Position::FACEUP_ATTACK, false),
        };

        if let Some(cm) = self.card_mut(card_id) {
            cm.prev_position = prev;
            cm.position = new_pos;
            cm.status.insert(CardStatus::FORM_CHANGED);
            if is_flip {
                cm.status.insert(CardStatus::FLIP_SUMMON_TURN | CardStatus::EFFECT_ENABLED);
                cm.summon_type = SummonType::FLIP;
            }
        }

        if is_flip {
            self.state.players[player as usize].summons_this_turn += 1;
            self.emit(DuelMessage::FlipSummoning {
                card_id,
                code: c.data.code,
                controller: player,
                sequence: c.sequence,
            });
            self.emit(DuelMessage::FlipSummoned { card_id });
            self.raise_event(DuelEvent::new(event_code::FLIP, player).with_cards(vec![card_id]));
            self.raise_event(DuelEvent::new(event_code::FLIP_SUMMON_SUCCESS, player).with_cards(vec![card_id]));
        } else {
            self.emit(DuelMessage::PosChange {
                card_id,
                code: c.data.code,
                controller: player,
                location: Location::MZONE,
                sequence: c.sequence,
                prev_position: prev,
                new_position: new_pos,
            });
            self.raise_event(DuelEvent::new(event_code::CHANGE_POS, player).with_cards(vec![card_id]));
        }
        self.recalculate_effects(host);
        Ok(())
    }

    /// Set a Spell/Trap card from Hand into S/T Zone (or Field Spell Zone).
    pub fn set_spell_trap(
        &mut self,
        player: PlayerId,
        card_id: CardId,
        zone: Option<u8>,
        host: &mut dyn ScriptHost,
    ) -> Result<(), DuelError> {
        if self.state.turn_player != player || !self.state.phase.is_main_phase() {
            return Err(DuelError::IllegalAction("can only Set Spell/Trap in Main Phase".into()));
        }
        let c = self.card(card_id).ok_or(DuelError::CardNotFound(card_id))?.clone();
        if c.location != Location::HAND || c.controller != player || (!c.data.is_spell() && !c.data.is_trap()) {
            return Err(DuelError::IllegalAction("must be a Spell/Trap in your hand".into()));
        }

        let (target_loc, target_seq) = if c.data.card_type.contains(CardType::FIELD) {
            self.clear_field_spell_slot(player, host);
            (Location::FZONE, 5u8)
        } else {
            let z = match zone {
                Some(s) if s < self.state.config.stz_count() as u8 && self.card_at(player, Location::SZONE, s).is_none() => s,
                Some(s) => return Err(DuelError::InvalidZone(s)),
                None => *self
                    .free_stz_zones(player)
                    .first()
                    .ok_or_else(|| DuelError::IllegalAction("no free S/T zone".into()))?,
            };
            (Location::SZONE, z)
        };

        self.move_card(
            card_id,
            player,
            target_loc,
            Some(target_seq),
            Position::FACEDOWN_ATTACK,
            Reason::RULE,
            player,
            None,
            host,
        )?;
        if let Some(cm) = self.card_mut(card_id) {
            cm.status.insert(CardStatus::SET_TURN);
        }
        self.emit(DuelMessage::Set {
            card_id,
            controller: player,
            location: target_loc,
            sequence: target_seq,
            position: Position::FACEDOWN_ATTACK,
        });
        self.raise_event(DuelEvent::new(event_code::SSET, player).with_cards(vec![card_id]));
        Ok(())
    }

    /// Place a Pendulum Monster from Hand into Left or Right Pendulum Scale Zone!
    pub fn activate_pendulum_scale(
        &mut self,
        player: PlayerId,
        card_id: CardId,
        right_side: bool,
        host: &mut dyn ScriptHost,
    ) -> Result<(), DuelError> {
        if self.state.config.flags.contains(DuelFlags::NO_PENDULUM_OR_LINK) {
            return Err(DuelError::IllegalAction("Pendulum summoning is disabled in this format".into()));
        }
        if self.state.turn_player != player || !self.state.phase.is_main_phase() {
            return Err(DuelError::IllegalAction("Pendulum Scales can only be placed in Main Phase".into()));
        }
        let c = self.card(card_id).ok_or(DuelError::CardNotFound(card_id))?.clone();
        if !c.data.is_pendulum() || c.location != Location::HAND || c.controller != player {
            return Err(DuelError::IllegalAction("card must be a Pendulum Monster in hand".into()));
        }
        let (lseq, rseq) = self.state.config.pendulum_zone_seqs();
        let target_seq = if right_side { rseq } else { lseq };
        if self.card_at(player, Location::PZONE, target_seq).is_some()
            || self.card_at(player, Location::SZONE, target_seq).is_some()
        {
            return Err(DuelError::InvalidZone(target_seq));
        }

        self.move_card(
            card_id,
            player,
            Location::PZONE,
            Some(target_seq),
            Position::FACEUP_ATTACK,
            Reason::RULE,
            player,
            None,
            host,
        )?;
        self.recalculate_effects(host);
        Ok(())
    }

    /// Core Special Summon executor enforcing:
    /// - `CANNOT_SPECIAL_SUMMON` floodgate
    /// - Proper Special Summon / Revival Rule (`STATUS_PROC_COMPLETE`)!
    /// - Master Rule 1-5 Extra Deck EMZ/Linked Zone rules & Extra Link!
    pub fn special_summon(
        &mut self,
        player: PlayerId,
        card_id: CardId,
        summon_type: SummonType,
        position: Position,
        zone: Option<u8>,
        ignore_revive_limit: bool,
        mark_proc_complete: bool,
        host: &mut dyn ScriptHost,
    ) -> Result<(), DuelError> {
        let team = self.team_of(player);
        if self.state.floodgates.cannot_special_summon[team] {
            return Err(DuelError::IllegalAction("Special Summoning is prevented by an active effect".into()));
        }
        let c = self.card(card_id).ok_or(DuelError::CardNotFound(card_id))?.clone();
        if !c.data.is_monster() {
            return Err(DuelError::IllegalAction("only monsters can be Special Summoned".into()));
        }
        if c.data.is_link() && position.intersects(Position::DEFENSE) {
            return Err(DuelError::IllegalAction("Link Monsters cannot be summoned in Defense Position".into()));
        }
        if self.state.config.flags.contains(DuelFlags::NO_PENDULUM_OR_LINK)
            && (c.data.is_link() || c.data.is_pendulum())
        {
            return Err(DuelError::IllegalAction("Link/Pendulum summoning disabled in this format".into()));
        }

        // Official Proper Special Summon / Revival Limit Rule!
        if (c.location == Location::GRAVE || c.location == Location::REMOVED)
            && c.requires_proper_summon()
            && !c.is_properly_summoned()
            && !ignore_revive_limit
        {
            return Err(DuelError::IllegalAction(format!(
                "{} was not properly Special Summoned first and cannot be revived from {:?}",
                c.data.name, c.location
            )));
        }

        let from_extra = c.location == Location::EXTRA;
        let allowed_zones = if from_extra {
            self.valid_extra_deck_zones(player, &c.data)
        } else {
            self.free_mmz_zones(player)
        };

        let target_seq = match zone {
            Some(z) if allowed_zones.contains(&z) => z,
            Some(z) => return Err(DuelError::InvalidZone(z)),
            None => *allowed_zones
                .first()
                .ok_or_else(|| DuelError::IllegalAction("no valid Monster Zone available for Special Summon".into()))?,
        };

        let from_loc = c.location;
        self.move_card(
            card_id,
            player,
            Location::MZONE,
            Some(target_seq),
            position,
            Reason::SPSUMMON,
            player,
            None,
            host,
        )?;
        let turn_id = self.state.turn_count;
        if let Some(cm) = self.card_mut(card_id) {
            cm.summon_type = summon_type | SummonType::SPECIAL;
            cm.summon_player = player;
            cm.summon_location = from_loc;
            cm.turn_id = turn_id;
            cm.status.insert(CardStatus::SPSUMMON_TURN);
            if position.intersects(Position::FACEUP) {
                cm.status.insert(CardStatus::EFFECT_ENABLED);
            }
            if mark_proc_complete {
                cm.status.insert(CardStatus::PROC_COMPLETE);
            }
        }
        self.state.players[player as usize].summons_this_turn += 1;

        self.emit(DuelMessage::SpSummoning {
            card_id,
            code: c.data.code,
            controller: player,
            sequence: target_seq,
            position,
            summon_type: summon_type | SummonType::SPECIAL,
        });
        self.emit(DuelMessage::SpSummoned {
            card_id,
            summon_type: summon_type | SummonType::SPECIAL,
        });
        self.raise_event(
            DuelEvent::new(event_code::SPSUMMON_SUCCESS, player)
                .with_cards(vec![card_id])
                .with_reason(Reason::SPSUMMON, player, None),
        );
        self.recalculate_effects(host);
        Ok(())
    }

    /// **Synchro Summon**: requires >=1 Tuner + >=1 non-Tuner face-up on field whose Levels sum exactly to Synchro's Level!
    pub fn synchro_summon(
        &mut self,
        player: PlayerId,
        synchro_id: CardId,
        materials: &[CardId],
        zone: Option<u8>,
        host: &mut dyn ScriptHost,
    ) -> Result<(), DuelError> {
        let syn = self.card(synchro_id).ok_or(DuelError::CardNotFound(synchro_id))?.clone();
        if !syn.data.card_type.contains(CardType::SYNCHRO) || syn.location != Location::EXTRA {
            return Err(DuelError::IllegalAction("not a Synchro Monster in Extra Deck".into()));
        }
        if materials.len() < 2 {
            return Err(DuelError::IllegalAction("Synchro Summon requires at least 2 materials".into()));
        }
        let mut tuners = 0usize;
        let mut non_tuners = 0usize;
        let mut level_sum = 0u32;
        for &mid in materials {
            let m = self.card(mid).ok_or(DuelError::CardNotFound(mid))?;
            if m.location != Location::MZONE || !m.is_faceup() || self.team_of(m.controller) != self.team_of(player) {
                return Err(DuelError::IllegalAction("Synchro materials must be face-up monsters you control".into()));
            }
            if m.data.is_xyz() || m.data.is_link() {
                return Err(DuelError::IllegalAction("Xyz/Link monsters have no Level for Synchro Summon".into()));
            }
            if m.data.is_tuner() {
                tuners += 1;
            } else {
                non_tuners += 1;
            }
            level_sum += m.computed.level;
        }
        if tuners < 1 || non_tuners < 1 || level_sum != syn.data.level {
            return Err(DuelError::IllegalAction(format!(
                "invalid Synchro materials: tuners={tuners}, non_tuners={non_tuners}, sum={level_sum} != {}",
                syn.data.level
            )));
        }

        // Send materials to GY (with REASON_SYNCHRO | REASON_MATERIAL)
        for &mid in materials {
            let owner = self.card(mid).unwrap().owner;
            self.move_card(
                mid,
                owner,
                Location::GRAVE,
                None,
                Position::FACEUP_ATTACK,
                Reason::SYNCHRO | Reason::MATERIAL,
                player,
                None,
                host,
            )?;
        }
        self.raise_event(
            DuelEvent::new(event_code::BE_MATERIAL, player)
                .with_cards(materials.to_vec())
                .with_reason(Reason::SYNCHRO | Reason::MATERIAL, player, None),
        );

        self.special_summon(
            player,
            synchro_id,
            SummonType::SYNCHRO,
            Position::FACEUP_ATTACK,
            zone,
            true,
            true,
            host,
        )
    }

    /// **Xyz Summon**: requires >=2 face-up non-Token monsters of the same Level == Xyz Rank, attaches them as Overlay Units!
    pub fn xyz_summon(
        &mut self,
        player: PlayerId,
        xyz_id: CardId,
        materials: &[CardId],
        zone: Option<u8>,
        host: &mut dyn ScriptHost,
    ) -> Result<(), DuelError> {
        let xyz = self.card(xyz_id).ok_or(DuelError::CardNotFound(xyz_id))?.clone();
        if !xyz.data.is_xyz() || xyz.location != Location::EXTRA {
            return Err(DuelError::IllegalAction("not an Xyz Monster in Extra Deck".into()));
        }
        if materials.len() < 2 {
            return Err(DuelError::IllegalAction("Xyz Summon requires at least 2 materials".into()));
        }
        for &mid in materials {
            let m = self.card(mid).ok_or(DuelError::CardNotFound(mid))?;
            if m.location != Location::MZONE || !m.is_faceup() || self.team_of(m.controller) != self.team_of(player) {
                return Err(DuelError::IllegalAction("Xyz materials must be face-up monsters you control".into()));
            }
            if m.data.card_type.contains(CardType::TOKEN) {
                return Err(DuelError::IllegalAction("Tokens cannot be used as Xyz Materials".into()));
            }
            if m.computed.level != xyz.data.level {
                return Err(DuelError::IllegalAction(format!(
                    "Xyz material level {} != Rank {}",
                    m.computed.level, xyz.data.level
                )));
            }
        }

        // Attach materials as Overlay Units (NOT sent to GY, NOT leaving field!)
        for (idx, &mid) in materials.iter().enumerate() {
            if let Some(m) = self.card_mut(mid) {
                m.prev_location = m.location;
                m.prev_sequence = m.sequence;
                m.location = Location::OVERLAY;
                m.sequence = idx as u8;
                m.overlay_parent = Some(xyz_id);
                m.counters.clear();
            }
        }
        if let Some(x) = self.card_mut(xyz_id) {
            x.overlay_materials.extend_from_slice(materials);
        }

        self.special_summon(
            player,
            xyz_id,
            SummonType::XYZ,
            Position::FACEUP_ATTACK,
            zone,
            true,
            true,
            host,
        )
    }

    /// Detach `count` Xyz Materials from an Xyz Monster to GY (does NOT trigger `LEAVE_FIELD`).
    pub fn detach_overlay(
        &mut self,
        xyz_id: CardId,
        count: usize,
        reason: Reason,
        player: PlayerId,
        host: &mut dyn ScriptHost,
    ) -> Result<Vec<CardId>, DuelError> {
        let mats = self
            .card(xyz_id)
            .ok_or(DuelError::CardNotFound(xyz_id))?
            .overlay_materials
            .clone();
        if mats.len() < count {
            return Err(DuelError::IllegalAction("not enough Xyz Materials to detach".into()));
        }
        let mut detached = Vec::with_capacity(count);
        for _ in 0..count {
            let mat_id = self.card_mut(xyz_id).unwrap().overlay_materials.pop().unwrap();
            if let Some(m) = self.card_mut(mat_id) {
                m.overlay_parent = None;
            }
            let owner = self.card(mat_id).unwrap().owner;
            let gy_seq = self.cards_in(owner, Location::GRAVE).len() as u8;
            self.move_card_raw(
                mat_id,
                owner,
                Location::GRAVE,
                gy_seq,
                Position::FACEUP_ATTACK,
                reason,
                player,
                None,
                host,
            );
            self.raise_event(
                DuelEvent::new(event_code::TO_GRAVE, owner)
                    .with_cards(vec![mat_id])
                    .with_reason(reason, player, None),
            );
            detached.push(mat_id);
        }
        self.recalculate_effects(host);
        Ok(detached)
    }

    /// **Link Summon**: exact Link Rating match where Link Monsters count as `1` OR their own Link Rating!
    pub fn link_summon(
        &mut self,
        player: PlayerId,
        link_id: CardId,
        materials: &[CardId],
        zone: Option<u8>,
        host: &mut dyn ScriptHost,
    ) -> Result<(), DuelError> {
        if self.state.config.flags.contains(DuelFlags::NO_PENDULUM_OR_LINK) {
            return Err(DuelError::IllegalAction("Link Summon disabled in this format".into()));
        }
        let link = self.card(link_id).ok_or(DuelError::CardNotFound(link_id))?.clone();
        if !link.data.is_link() || link.location != Location::EXTRA {
            return Err(DuelError::IllegalAction("not a Link Monster in Extra Deck".into()));
        }
        let target_rating = link.data.level;
        let mut ratings = Vec::with_capacity(materials.len());
        for &mid in materials {
            let m = self.card(mid).ok_or(DuelError::CardNotFound(mid))?;
            if m.location != Location::MZONE || !m.is_faceup() || self.team_of(m.controller) != self.team_of(player) {
                return Err(DuelError::IllegalAction("Link materials must be face-up monsters you control".into()));
            }
            ratings.push(if m.data.is_link() { m.data.level } else { 1 });
        }
        if !can_match_link_rating(&ratings, target_rating) {
            return Err(DuelError::IllegalAction(format!(
                "materials {:?} cannot sum to Link Rating {target_rating}",
                ratings
            )));
        }

        // Send materials to GY first so zones freed by materials can be used!
        for &mid in materials {
            let owner = self.card(mid).unwrap().owner;
            self.move_card(
                mid,
                owner,
                Location::GRAVE,
                None,
                Position::FACEUP_ATTACK,
                Reason::LINK | Reason::MATERIAL,
                player,
                None,
                host,
            )?;
        }
        self.raise_event(
            DuelEvent::new(event_code::BE_MATERIAL, player)
                .with_cards(materials.to_vec())
                .with_reason(Reason::LINK | Reason::MATERIAL, player, None),
        );

        self.special_summon(
            player,
            link_id,
            SummonType::LINK,
            Position::FACEUP_ATTACK,
            zone,
            true,
            true,
            host,
        )
    }

    /// **Fusion Summon**: sends fusion materials to GY and Special Summons Fusion Monster (`STATUS_PROC_COMPLETE`).
    pub fn fusion_summon(
        &mut self,
        player: PlayerId,
        fusion_id: CardId,
        materials: &[CardId],
        zone: Option<u8>,
        host: &mut dyn ScriptHost,
    ) -> Result<(), DuelError> {
        let fus = self.card(fusion_id).ok_or(DuelError::CardNotFound(fusion_id))?.clone();
        if !fus.data.card_type.contains(CardType::FUSION) || fus.location != Location::EXTRA {
            return Err(DuelError::IllegalAction("not a Fusion Monster in Extra Deck".into()));
        }
        if materials.is_empty() {
            return Err(DuelError::IllegalAction("Fusion Summon requires materials".into()));
        }
        for &mid in materials {
            let owner = self.card(mid).ok_or(DuelError::CardNotFound(mid))?.owner;
            self.move_card(
                mid,
                owner,
                Location::GRAVE,
                None,
                Position::FACEUP_ATTACK,
                Reason::FUSION | Reason::MATERIAL,
                player,
                None,
                host,
            )?;
        }
        self.special_summon(
            player,
            fusion_id,
            SummonType::FUSION,
            Position::FACEUP_ATTACK,
            zone,
            true,
            true,
            host,
        )
    }

    /// **Ritual Summon**: tributes monsters whose total Levels >= Ritual Monster's Level.
    pub fn ritual_summon(
        &mut self,
        player: PlayerId,
        ritual_id: CardId,
        tributes: &[CardId],
        zone: Option<u8>,
        host: &mut dyn ScriptHost,
    ) -> Result<(), DuelError> {
        let rit = self.card(ritual_id).ok_or(DuelError::CardNotFound(ritual_id))?.clone();
        if !rit.data.is_ritual() {
            return Err(DuelError::IllegalAction("not a Ritual Monster".into()));
        }
        let sum: u32 = tributes
            .iter()
            .filter_map(|&id| self.card(id))
            .map(|c| c.computed.level)
            .sum();
        if sum < rit.data.level {
            return Err(DuelError::IllegalAction(format!(
                "Ritual tributes sum {sum} < Level {}",
                rit.data.level
            )));
        }
        for &tid in tributes {
            let owner = self.card(tid).ok_or(DuelError::CardNotFound(tid))?.owner;
            self.move_card(
                tid,
                owner,
                Location::GRAVE,
                None,
                Position::FACEUP_ATTACK,
                Reason::RITUAL | Reason::RELEASE | Reason::MATERIAL,
                player,
                None,
                host,
            )?;
        }
        self.special_summon(
            player,
            ritual_id,
            SummonType::RITUAL,
            Position::FACEUP_ATTACK,
            zone,
            true,
            true,
            host,
        )
    }

    /// **Pendulum Summon**: once per turn, simultaneously Special Summon monsters with `min_scale < Level < max_scale`
    /// from Hand (to MMZ) and face-up Extra Deck (to EMZ / Linked MMZ in MR4/MR5)!
    pub fn pendulum_summon(
        &mut self,
        player: PlayerId,
        cards: &[CardId],
        host: &mut dyn ScriptHost,
    ) -> Result<Vec<CardId>, DuelError> {
        if self.state.config.flags.contains(DuelFlags::NO_PENDULUM_OR_LINK) {
            return Err(DuelError::IllegalAction("Pendulum Summon disabled in this format".into()));
        }
        if self.state.turn_player != player || !self.state.phase.is_main_phase() {
            return Err(DuelError::IllegalAction("Pendulum Summon only in your Main Phase".into()));
        }
        if self.state.players[player as usize].pendulum_summoned_this_turn {
            return Err(DuelError::IllegalAction("already Pendulum Summoned this turn".into()));
        }
        let (ls, rs) = self
            .pendulum_scales(player)
            .ok_or_else(|| DuelError::IllegalAction("need two Pendulum Scales set".into()))?;
        let (lo, hi) = if ls < rs { (ls, rs) } else { (rs, ls) };
        if hi <= lo + 1 {
            return Err(DuelError::IllegalAction("no valid Levels between Pendulum Scales".into()));
        }
        if cards.is_empty() {
            return Err(DuelError::IllegalAction("must choose at least 1 monster to Pendulum Summon".into()));
        }

        // Validate every chosen card first
        for &cid in cards {
            let c = self.card(cid).ok_or(DuelError::CardNotFound(cid))?;
            let valid_loc = (c.location == Location::HAND && c.controller == player)
                || (c.location == Location::EXTRA && c.owner == player && c.is_faceup() && c.data.is_pendulum());
            if !valid_loc || !c.data.is_monster() {
                return Err(DuelError::IllegalAction("invalid Pendulum Summon candidate".into()));
            }
            if c.computed.level <= lo || c.computed.level >= hi {
                return Err(DuelError::IllegalAction(format!(
                    "{} Level {} not strictly between {} and {}",
                    c.data.name, c.computed.level, lo, hi
                )));
            }
        }

        let mut summoned = Vec::new();
        for &cid in cards {
            self.special_summon(
                player,
                cid,
                SummonType::PENDULUM,
                Position::FACEUP_ATTACK,
                None,
                false,
                true,
                host,
            )?;
            summoned.push(cid);
        }
        self.state.players[player as usize].pendulum_summoned_this_turn = true;
        Ok(summoned)
    }

    /// **Rush Duel Maximum Summon**: sends existing MMZ monsters to GY and summons `[Left, Center, Right]`
    /// simultaneously in Maximum Mode with combined Maximum ATK!
    pub fn maximum_summon(
        &mut self,
        player: PlayerId,
        pieces: [CardId; 3],
        host: &mut dyn ScriptHost,
    ) -> Result<(), DuelError> {
        if !self.state.config.is_rush() {
            return Err(DuelError::IllegalAction("Maximum Summon is only legal in Rush Duel".into()));
        }
        if self.state.turn_player != player || !self.state.phase.is_main_phase() {
            return Err(DuelError::IllegalAction("Maximum Summon only in Main Phase".into()));
        }
        for &pid in &pieces {
            let c = self.card(pid).ok_or(DuelError::CardNotFound(pid))?;
            if c.location != Location::HAND || c.controller != player || !c.data.is_maximum() {
                return Err(DuelError::IllegalAction("all 3 pieces must be Maximum Monsters in hand".into()));
            }
        }
        // Send existing monsters in MMZ to GY
        let existing = self.cards_in(player, Location::MZONE);
        for eid in existing {
            let owner = self.card(eid).unwrap().owner;
            self.move_card(
                eid,
                owner,
                Location::GRAVE,
                None,
                Position::FACEUP_ATTACK,
                Reason::RULE,
                player,
                None,
                host,
            )?;
        }
        // Summon Left (0), Center (1), Right (2)
        for (seq, &pid) in pieces.iter().enumerate() {
            self.special_summon(
                player,
                pid,
                SummonType::MAXIMUM,
                Position::FACEUP_ATTACK,
                Some(seq as u8),
                true,
                true,
                host,
            )?;
            if let Some(c) = self.card_mut(pid) {
                c.status.insert(CardStatus::MAXIMUM_MODE);
                c.maximum_pieces = pieces.to_vec();
            }
        }
        self.recalculate_effects(host);
        Ok(())
    }

    /// Activate a Speed Duel Skill Card!
    pub fn activate_skill(&mut self, player: PlayerId, skill_id: CardId, host: &mut dyn ScriptHost) -> Result<(), DuelError> {
        if !self.state.config.is_speed() {
            return Err(DuelError::IllegalAction("Skill Cards are only legal in Speed Duel".into()));
        }
        let sk = self.card(skill_id).ok_or(DuelError::CardNotFound(skill_id))?.clone();
        if sk.location != Location::SKILL || sk.owner != player {
            return Err(DuelError::IllegalAction("invalid Skill Card".into()));
        }
        if let Some(c) = self.card_mut(skill_id) {
            c.position = Position::FACEUP_ATTACK;
            c.status.insert(CardStatus::EFFECT_ENABLED);
        }
        self.state.players[player as usize].skill_used = true;
        self.emit(DuelMessage::SkillActivated {
            player,
            card_id: skill_id,
            name: sk.data.name,
        });
        // Execute any registered ACTIVATE effect on the Skill Card directly (Skills do not start a chain!)
        let eff_ids = sk.effects.clone();
        for eid in eff_ids {
            if let Some(eff) = self.state.effects.get(&eid).cloned() {
                if eff.effect_type.contains(EffectType::ACTIVATE) {
                    host.eval_operation(self, eid, player, None);
                }
            }
        }
        self.recalculate_effects(host);
        Ok(())
    }

    // -------------------------------------------------------------------------
    // CHAIN SYSTEM, SEGOC, MISSED TIMING, SPELL SPEED, ACTIVATION & RESOLUTION
    // -------------------------------------------------------------------------

    pub fn raise_event(&mut self, mut event: DuelEvent) {
        // Any prior events in this same batch are no longer the last thing to happen!
        for prev in &mut self.state.pending_events {
            prev.is_last_thing_to_happen = false;
        }
        event.is_last_thing_to_happen = true;
        self.state.pending_events.push(event);
    }

    /// Check if an effect's OPT (Soft OPT, Hard OPT, Oath, Once Per Duel, Once Per Chain) allows activation.
    pub fn check_count_limit(&self, eff: &Effect, player: PlayerId) -> bool {
        let Some(cl) = &eff.count_limit else {
            return true;
        };
        if cl.is_chain && cl.used_this_chain >= cl.max_count {
            return false;
        }
        if cl.code != 0 {
            let p = &self.state.players[player as usize];
            if cl.is_duel {
                return p.opt_used_this_duel.get(&cl.code).copied().unwrap_or(0) < cl.max_count;
            }
            return p.opt_used_this_turn.get(&cl.code).copied().unwrap_or(0) < cl.max_count;
        }
        cl.used_this_turn < cl.max_count
    }

    fn consume_count_limit(&mut self, effect_id: EffectId, player: PlayerId) {
        let Some(cl) = self.state.effects.get(&effect_id).and_then(|e| e.count_limit.clone()) else {
            return;
        };
        if let Some(eff) = self.state.effects.get_mut(&effect_id) {
            if let Some(c) = &mut eff.count_limit {
                c.used_this_turn += 1;
                c.used_this_chain += 1;
            }
        }
        if cl.code != 0 {
            let p = &mut self.state.players[player as usize];
            if cl.is_duel {
                *p.opt_used_this_duel.entry(cl.code).or_insert(0) += 1;
            } else {
                *p.opt_used_this_turn.entry(cl.code).or_insert(0) += 1;
            }
        }
    }

    /// Refund Oath OPT if activation was negated! (Non-oath "only use" Hard OPT is NOT refunded!)
    fn refund_oath_if_applicable(&mut self, effect_id: EffectId, player: PlayerId) {
        let Some(cl) = self.state.effects.get(&effect_id).and_then(|e| e.count_limit.clone()) else {
            return;
        };
        if !cl.is_oath {
            return;
        }
        if let Some(eff) = self.state.effects.get_mut(&effect_id) {
            if let Some(c) = &mut eff.count_limit {
                c.used_this_turn = c.used_this_turn.saturating_sub(1);
            }
        }
        if cl.code != 0 {
            let p = &mut self.state.players[player as usize];
            if let Some(v) = p.opt_used_this_turn.get_mut(&cl.code) {
                *v = v.saturating_sub(1);
            }
        }
    }

    /// Collect all pending SEGOC triggers from `pending_events`, enforcing **"When... you can" Missed Timing**
    /// vs **"If... you can" (`EFFECT_FLAG_DELAY`) Never Misses Timing**!
    pub fn collect_segoc_triggers(&mut self, host: &mut dyn ScriptHost) -> Vec<PendingTrigger> {
        let events = std::mem::take(&mut self.state.pending_events);
        if events.is_empty() {
            return Vec::new();
        }
        let all_effects: Vec<Effect> = self
            .state
            .effects
            .values()
            .filter(|e| {
                e.active
                    && e.effect_type.intersects(
                        EffectType::TRIGGER_F
                            | EffectType::TRIGGER_O
                            | EffectType::FLIP
                            | EffectType::QUICK_F,
                    )
            })
            .cloned()
            .collect();

        let mut triggers = Vec::new();
        for ev in &events {
            for eff in &all_effects {
                if eff.code != ev.code && !(eff.effect_type.contains(EffectType::FLIP) && ev.code == event_code::FLIP) {
                    continue;
                }
                let Some(cid) = eff.handler_card else {
                    continue;
                };
                let Some(card) = self.card(cid).cloned() else {
                    continue;
                };
                if !eff.range.is_empty() && !card.location.intersects(eff.range) {
                    continue;
                }
                if eff.effect_type.contains(EffectType::SINGLE) && !ev.cards.contains(&cid) {
                    continue;
                }
                // OFFICIAL MISSED TIMING CHECK!
                // If optional "When... you can" (`can_miss_timing()`) AND event was NOT the last thing to happen,
                // it MISSES TIMING! Mandatory triggers & "If... you can" (`EFFECT_FLAG_DELAY`) NEVER miss timing!
                if eff.can_miss_timing() && !ev.is_last_thing_to_happen {
                    continue;
                }
                if !self.check_count_limit(eff, card.controller) {
                    continue;
                }
                if !self.check_effect_condition(eff, card.controller, Some(ev), host) {
                    continue;
                }
                triggers.push(PendingTrigger {
                    effect_id: eff.id,
                    card_id: cid,
                    controller: card.controller,
                    mandatory: eff.is_mandatory_trigger(),
                    spell_speed: eff.spell_speed(card.data.card_type),
                    description: eff.description,
                });
            }
        }

        sort_segoc_triggers(&mut triggers, self.state.turn_player);
        self.state.pending_triggers = triggers.clone();
        triggers
    }

    /// Check whether a player can activate `(card_id, effect_id)` right now!
    pub fn can_activate_effect(
        &mut self,
        player: PlayerId,
        card_id: CardId,
        effect_id: EffectId,
        host: &mut dyn ScriptHost,
    ) -> bool {
        let Some(card) = self.card(card_id).cloned() else {
            return false;
        };
        let Some(eff) = self.state.effects.get(&effect_id).cloned() else {
            return false;
        };
        if !eff.active || !eff.is_activated() {
            return false;
        }
        if self.team_of(card.controller) != self.team_of(player) && !eff.flags.contains(EffectFlag::BOTH_SIDE) {
            return false;
        }

        // Check floodgates
        let team = self.team_of(player);
        if card.data.is_monster() && self.state.floodgates.cannot_activate_monster_effects[team] {
            return false;
        }
        if card.data.is_spell() && self.state.floodgates.cannot_activate_spells[team] {
            return false;
        }
        if card.data.is_trap() && self.state.floodgates.cannot_activate_traps[team] {
            return false;
        }

        // Check range / Spell-Trap activation legality
        if eff.effect_type.contains(EffectType::ACTIVATE) && (card.data.is_spell() || card.data.is_trap()) {
            let from_hand = card.location == Location::HAND;
            let from_set = card.location.intersects(Location::SZONE | Location::FZONE) && card.is_facedown();
            if !from_hand && !from_set {
                return false;
            }
            if from_hand {
                if card.data.is_trap() {
                    // Traps cannot be activated from hand unless an active TRAP_ACT_IN_HAND effect applies!
                    let can_hand_trap = self.has_effect_code_on_card(card_id, effect_code::TRAP_ACT_IN_HAND, host);
                    if !can_hand_trap {
                        return false;
                    }
                } else if card.data.is_spell() {
                    if self.state.turn_player != player {
                        return false;
                    }
                    if !card.data.card_type.contains(CardType::QUICKPLAY) && (!self.state.phase.is_main_phase() || !self.state.chain.is_empty()) {
                        return false;
                    }
                }
                if !card.data.card_type.contains(CardType::FIELD) && self.free_stz_zones(player).is_empty() {
                    return false;
                }
            }
            if from_set {
                // Traps & Quick-Play Spells cannot be activated the same turn they were Set!
                if card.status.contains(CardStatus::SET_TURN)
                    && (card.data.is_trap() || card.data.card_type.contains(CardType::QUICKPLAY))
                {
                    let bypass = self.has_effect_code_on_card(card_id, effect_code::TRAP_ACT_IN_SET_TURN, host)
                        || self.has_effect_code_on_card(card_id, effect_code::QP_ACT_IN_SET_TURN, host);
                    if !bypass {
                        return false;
                    }
                }
            }
        } else {
            if !eff.range.is_empty() && !card.location.intersects(eff.range) {
                return false;
            }
            if card.is_on_field() && !card.is_faceup() && !eff.flags.contains(EffectFlag::SET_AVAILABLE) {
                return false;
            }
            if card.data.card_type.contains(CardType::GEMINI) && !card.is_gemini_effect_active() && card.location == Location::MZONE {
                return false;
            }
            if eff.effect_type.contains(EffectType::IGNITION) {
                if self.state.turn_player != player || !self.state.phase.is_main_phase() || !self.state.chain.is_empty() {
                    return false;
                }
            }
        }

        // Spell Speed & Chain validation
        let ss = eff.spell_speed(card.data.card_type);
        if self.state.config.is_rush() && !self.state.chain.is_empty() {
            return false; // Rush Duel has no Chains!
        }
        if !self.state.chain.can_chain(ss, player) {
            return false;
        }
        // Damage Step restriction: only ATK/DEF mods, Counter Traps (SS3), Trigger effects, or DAMAGE_STEP flagged effects!
        if self.state.battle.sub_step != DamageSubStep::None {
            let allowed_in_ds = ss == 3
                || eff.flags.intersects(EffectFlag::DAMAGE_STEP | EffectFlag::DAMAGE_CAL)
                || eff.is_mandatory_trigger()
                || eff.effect_type.contains(EffectType::FLIP)
                || eff.category.intersects(Category::ATKCHANGE | Category::DEFCHANGE);
            if !allowed_in_ds {
                return false;
            }
        }

        if !self.check_count_limit(&eff, player) {
            return false;
        }
        let ev_clone = self.state.last_resolved_event.clone();
        if !self.check_effect_condition(&eff, player, ev_clone.as_ref(), host) {
            return false;
        }
        if eff.cost.is_some() && !host.eval_cost(self, effect_id, player, ev_clone.as_ref(), 0) {
            return false;
        }
        if eff.target.is_some() && !host.eval_target(self, effect_id, player, ev_clone.as_ref(), 0) {
            return false;
        }
        true
    }

    fn has_effect_code_on_card(&mut self, card_id: CardId, code: u32, host: &mut dyn ScriptHost) -> bool {
        let effs: Vec<Effect> = self
            .state
            .effects
            .values()
            .filter(|e| e.active && e.handler_card == Some(card_id) && e.code == code)
            .cloned()
            .collect();
        for eff in effs {
            if self.check_effect_condition(&eff, eff.owner_player, None, host) {
                return true;
            }
        }
        false
    }

    /// Activate an effect onto the Chain (paying costs, choosing targets, placing Spell/Trap on field).
    pub fn activate_effect(
        &mut self,
        player: PlayerId,
        card_id: CardId,
        effect_id: EffectId,
        targets: Vec<CardId>,
        host: &mut dyn ScriptHost,
    ) -> Result<u8, DuelError> {
        if !self.can_activate_effect(player, card_id, effect_id, host) {
            return Err(DuelError::IllegalAction("effect cannot be activated right now".into()));
        }
        let card = self.card(card_id).ok_or(DuelError::CardNotFound(card_id))?.clone();
        let eff = self.state.effects.get(&effect_id).ok_or(DuelError::EffectNotFound(effect_id))?.clone();

        // If activating a Spell/Trap from Hand or Set, move/flip it face-up onto the field
        if eff.effect_type.contains(EffectType::ACTIVATE) && (card.data.is_spell() || card.data.is_trap()) {
            if card.location == Location::HAND {
                if card.data.card_type.contains(CardType::FIELD) {
                    self.clear_field_spell_slot(player, host);
                    self.move_card(
                        card_id,
                        player,
                        Location::FZONE,
                        Some(5),
                        Position::FACEUP_ATTACK,
                        Reason::RULE,
                        player,
                        Some(effect_id),
                        host,
                    )?;
                } else {
                    self.move_card(
                        card_id,
                        player,
                        Location::SZONE,
                        None,
                        Position::FACEUP_ATTACK,
                        Reason::RULE,
                        player,
                        Some(effect_id),
                        host,
                    )?;
                }
            } else if card.is_facedown() {
                if let Some(cm) = self.card_mut(card_id) {
                    cm.prev_position = cm.position;
                    cm.position = Position::FACEUP_ATTACK;
                    cm.status.insert(CardStatus::EFFECT_ENABLED);
                }
                self.emit(DuelMessage::PosChange {
                    card_id,
                    code: card.data.code,
                    controller: card.controller,
                    location: card.location,
                    sequence: card.sequence,
                    prev_position: card.position,
                    new_position: Position::FACEUP_ATTACK,
                });
            }
        }

        // Consume OPT count limit
        self.consume_count_limit(effect_id, player);

        // Pay cost (`chk = 1`) and run target (`chk = 1`)
        self.state.current_chain_targets = targets.clone();
        let ev_clone = self.state.last_resolved_event.clone();
        if eff.cost.is_some() {
            let _ = host.eval_cost(self, effect_id, player, ev_clone.as_ref(), 1);
        }
        if eff.target.is_some() {
            let _ = host.eval_target(self, effect_id, player, ev_clone.as_ref(), 1);
        }
        let final_targets = self.state.current_chain_targets.clone();

        let card_now = self.card(card_id).cloned().unwrap_or(card);
        let ss = eff.spell_speed(card_now.data.card_type);
        let requires_faceup = card_now.data.card_type.intersects(
            CardType::CONTINUOUS | CardType::FIELD | CardType::EQUIP,
        ) || card_now.location == Location::PZONE;

        let link = ChainLink {
            link_number: 0,
            effect_id,
            card_id,
            triggering_player: player,
            triggering_location: card_now.location,
            triggering_sequence: card_now.sequence,
            triggering_position: card_now.position,
            spell_speed: ss,
            targets: final_targets.clone(),
            activation_negated: false,
            effect_negated: false,
            category: eff.category,
            label: eff.label,
            requires_faceup_on_field_to_resolve: requires_faceup,
        };
        let link_num = self.state.chain.push(link);

        self.emit(DuelMessage::Chaining {
            chain_link: link_num,
            card_id,
            code: card_now.data.code,
            controller: player,
            location: card_now.location,
            sequence: card_now.sequence,
            desc: eff.description,
        });
        self.emit(DuelMessage::Chained { chain_link: link_num });
        self.raise_event(
            DuelEvent::new(event_code::CHAINING, player)
                .with_cards(vec![card_id])
                .with_value(link_num as i64)
                .with_reason(Reason::EFFECT, player, Some(effect_id)),
        );
        if !final_targets.is_empty() {
            self.raise_event(
                DuelEvent::new(event_code::BECOME_TARGET, player)
                    .with_cards(final_targets)
                    .with_reason(Reason::EFFECT, player, Some(effect_id)),
            );
        }
        Ok(link_num)
    }

    /// Resolve the entire Chain Stack in exact LIFO (reverse) order!
    /// Tracks `is_last_thing_to_happen` accurately (only events from Chain Link 1 can be the last thing to happen!).
    pub fn resolve_chain(&mut self, host: &mut dyn ScriptHost) {
        if self.state.chain.is_empty() {
            return;
        }
        let total = self.state.chain.len();
        let mut to_grave_after_chain: Vec<CardId> = Vec::new();
        let mut chain_events: Vec<DuelEvent> = Vec::new();

        for idx in (0..total).rev() {
            self.state.chain.resolving_index = Some(idx);
            let link = self.state.chain.links[idx].clone();
            let link_num = link.link_number;
            self.emit(DuelMessage::ChainSolving { chain_link: link_num });

            // Recompute continuous effects before each link resolves!
            self.recalculate_effects(host);

            let card_now = self.card(link.card_id).cloned();
            let eff_now = self.state.effects.get(&link.effect_id).cloned();

            let mut negated = link.activation_negated || link.effect_negated;
            // Check if monster effect is negated on field (Skill Drain / Veiler / Impermanence)!
            // Official Rule: Skill Drain only negates effects of monsters STILL face-up on field at resolution!
            if !negated {
                if let Some(ref c) = card_now {
                    if c.data.is_monster()
                        && link.triggering_location == Location::MZONE
                        && c.location == Location::MZONE
                        && c.is_faceup()
                        && c.is_disabled()
                    {
                        negated = true;
                    }
                }
            }
            // Official Rule: Continuous Spell/Trap, Field Spell, Equip Spell, and Pendulum Scales
            // MUST remain face-up on the field to resolve! (MST negates Continuous/Field/Equip!)
            if !negated && link.requires_faceup_on_field_to_resolve {
                let still_faceup_on_field = card_now
                    .as_ref()
                    .map(|c| c.is_on_field() && c.is_faceup())
                    .unwrap_or(false);
                if !still_faceup_on_field {
                    negated = true;
                }
            }

            if link.activation_negated {
                self.refund_oath_if_applicable(link.effect_id, link.triggering_player);
                self.emit(DuelMessage::ChainNegated { chain_link: link_num });
            } else if negated {
                self.emit(DuelMessage::ChainDisabled { chain_link: link_num });
            } else if let Some(eff) = eff_now {
                self.state.current_chain_targets = link.targets.clone();
                let pre_ev_len = self.state.pending_events.len();
                let last_ev = self.state.last_resolved_event.clone();
                match &eff.operation {
                    EffectCallback::LuaFunc(_) => {
                        host.eval_operation(self, eff.id, link.triggering_player, last_ev.as_ref());
                    }
                    EffectCallback::Builtin(name) => {
                        let op_name = name.clone();
                        self.run_builtin_operation(&op_name, &link, host);
                    }
                    EffectCallback::None => {}
                }
                // Official Timing Rule: Only events raised during Chain Link 1 (`idx == 0`)
                // can be `is_last_thing_to_happen == true`! Events in Chain Link 2+ CANNOT be last!
                let new_evs: Vec<DuelEvent> = self.state.pending_events.drain(pre_ev_len..).collect();
                for mut ev in new_evs {
                    if idx > 0 {
                        ev.is_last_thing_to_happen = false;
                    }
                    chain_events.push(ev);
                }
            }

            // Normal Spells, Quick-Play Spells, Ritual Spells, Normal Traps, Counter Traps go to GY after Chain ends!
            if let Some(ref c) = card_now {
                let is_normal_st = (c.data.is_spell() || c.data.is_trap())
                    && !c.data.card_type.intersects(CardType::CONTINUOUS | CardType::FIELD | CardType::EQUIP)
                    && c.location == Location::SZONE;
                if is_normal_st {
                    to_grave_after_chain.push(c.id);
                }
            }

            self.emit(DuelMessage::ChainSolved { chain_link: link_num });
            self.recalculate_effects(host);
            self.check_win_conditions();
            if self.is_finished() {
                break;
            }
        }

        // Send resolved Normal/Quick-Play/Ritual/Counter Spell/Traps to GY
        for cid in to_grave_after_chain {
            if let Some(c) = self.card(cid).cloned() {
                if c.location == Location::SZONE {
                    let _ = self.move_card(
                        cid,
                        c.owner,
                        Location::GRAVE,
                        None,
                        Position::FACEUP_ATTACK,
                        Reason::RULE,
                        c.controller,
                        None,
                        host,
                    );
                }
            }
        }

        // Reset Once-Per-Chain counters
        for eff in self.state.effects.values_mut() {
            if let Some(cl) = &mut eff.count_limit {
                cl.used_this_chain = 0;
            }
        }

        // Preserve events with exact `is_last_thing_to_happen` flags!
        // Note: Only the final event of Chain Link 1 keeps `is_last_thing_to_happen = true`.
        let mut last_cl1_idx = None;
        for (i, ev) in chain_events.iter().enumerate() {
            if ev.is_last_thing_to_happen {
                last_cl1_idx = Some(i);
            }
        }
        for (i, ev) in chain_events.iter_mut().enumerate() {
            ev.is_last_thing_to_happen = Some(i) == last_cl1_idx;
        }
        self.state.last_resolved_event = chain_events.last().cloned();
        self.state.pending_events.extend(chain_events);

        self.state.chain.clear();
        self.emit(DuelMessage::ChainEnd);
        self.recalculate_effects(host);
        self.check_win_conditions();
    }

    fn run_builtin_operation(&mut self, name: &str, link: &ChainLink, host: &mut dyn ScriptHost) {
        let tp = link.triggering_player;
        let opp = self.opponent_of(tp);
        match name {
            "draw_1" => {
                self.draw_cards(tp, 1, Reason::EFFECT, host);
            }
            "draw_2" => {
                self.draw_cards(tp, 2, Reason::EFFECT, host);
            }
            "destroy_targets" => {
                self.destroy_cards(&link.targets, Reason::EFFECT, tp, Some(link.effect_id), host);
            }
            "destroy_all_opp_monsters" => {
                let mons = self.cards_in(opp, Location::MZONE);
                self.destroy_cards(&mons, Reason::EFFECT, tp, Some(link.effect_id), host);
            }
            "destroy_all_opp_st" => {
                let mut sts = self.cards_in(opp, Location::SZONE);
                sts.extend(self.cards_in(opp, Location::FZONE));
                sts.extend(self.cards_in(opp, Location::PZONE));
                self.destroy_cards(&sts, Reason::EFFECT, tp, Some(link.effect_id), host);
            }
            "damage_800_opp" => {
                self.inflict_damage(opp, 800, false, Reason::EFFECT, host);
            }
            "recover_1000_self" => {
                self.recover_lp(tp, 1000, Reason::EFFECT, host);
            }
            "negate_prev_activation" => {
                if link.link_number >= 2 {
                    self.state.chain.negate_activation(link.link_number - 1);
                }
            }
            "negate_prev_effect" => {
                if link.link_number >= 2 {
                    self.state.chain.negate_effect(link.link_number - 1);
                }
            }
            "special_summon_target_from_gy" => {
                if let Some(&tid) = link.targets.first() {
                    let _ = self.special_summon(
                        tp,
                        tid,
                        SummonType::SPECIAL,
                        Position::FACEUP_ATTACK,
                        None,
                        false,
                        false,
                        host,
                    );
                }
            }
            _ => {}
        }
    }

    fn clear_field_spell_slot(&mut self, player: PlayerId, host: &mut dyn ScriptHost) {
        let mut to_clear = self.cards_in(player, Location::FZONE);
        if self.state.config.flags.contains(DuelFlags::SHARED_FIELD_SPELL) {
            let opp = self.opponent_of(player);
            to_clear.extend(self.cards_in(opp, Location::FZONE));
        }
        for fid in to_clear {
            let owner = self.card(fid).map(|c| c.owner).unwrap_or(player);
            let _ = self.move_card(
                fid,
                owner,
                Location::GRAVE,
                None,
                Position::FACEUP_ATTACK,
                Reason::RULE,
                player,
                None,
                host,
            );
        }
    }

    // -------------------------------------------------------------------------
    // BATTLE PHASE & OFFICIAL 5-SUBSTEP DAMAGE STEP
    // -------------------------------------------------------------------------

    pub fn can_enter_battle_phase(&self, player: PlayerId) -> bool {
        self.state.turn_player == player
            && self.state.phase == Phase::Main1
            && self.state.turn_count > 1
            && self.state.chain.is_empty()
    }

    pub fn can_declare_attack(&self, attacker_id: CardId) -> bool {
        if !matches!(self.state.phase, Phase::BattleStart | Phase::BattleStep) {
            return false;
        }
        let Some(c) = self.card(attacker_id) else {
            return false;
        };
        if c.location != Location::MZONE
            || self.team_of(c.controller) != self.team_of(self.state.turn_player)
            || !c.is_faceup()
            || !c.is_attack_pos()
            || c.computed.cannot_attack
        {
            return false;
        }
        let max_attacks = 1 + c.computed.extra_attacks;
        c.attacks_declared_this_turn < max_attacks
    }

    pub fn valid_attack_targets(&self, attacker_id: CardId) -> Vec<Option<CardId>> {
        if !self.can_declare_attack(attacker_id) {
            return Vec::new();
        }
        let Some(atk_card) = self.card(attacker_id) else {
            return Vec::new();
        };
        let opp = self.opponent_of(atk_card.controller);
        let opp_mons: Vec<CardId> = self
            .cards_in(opp, Location::MZONE)
            .into_iter()
            .filter(|&id| self.card(id).map(|c| !c.computed.untargetable_battle).unwrap_or(false))
            .collect();
        let mut targets: Vec<Option<CardId>> = opp_mons.iter().copied().map(Some).collect();
        if self.cards_in(opp, Location::MZONE).is_empty() || atk_card.computed.direct_attack {
            targets.push(None); // Direct attack!
        }
        targets
    }

    /// Declare an attack and run through the Battle Step & ALL 5 official Damage Step sub-steps!
    pub fn declare_attack(
        &mut self,
        attacker_id: CardId,
        target_id: Option<CardId>,
        host: &mut dyn ScriptHost,
    ) -> Result<(), DuelError> {
        if !self.can_declare_attack(attacker_id) {
            return Err(DuelError::IllegalAction("monster cannot declare an attack".into()));
        }
        let valid = self.valid_attack_targets(attacker_id);
        if !valid.contains(&target_id) {
            return Err(DuelError::IllegalAction("invalid attack target".into()));
        }
        let attacker = self.card(attacker_id).unwrap().clone();
        let opp = self.opponent_of(attacker.controller);

        self.state.phase = Phase::BattleStep;
        self.state.battle = BattleState {
            attacker: Some(attacker_id),
            defender: target_id,
            attack_negated: false,
            replay_available: false,
            sub_step: DamageSubStep::None,
            expected_opp_monster_count: self.cards_in(opp, Location::MZONE).len(),
            destroyed_in_battle: Vec::new(),
            flipped_in_damage_step: None,
        };
        if let Some(a) = self.card_mut(attacker_id) {
            a.attacks_declared_this_turn += 1;
        }

        self.emit(DuelMessage::Attack {
            attacker: attacker_id,
            target: target_id,
        });
        self.raise_event(DuelEvent::new(event_code::ATTACK_ANNOUNCE, attacker.controller).with_cards(vec![attacker_id]));
        if let Some(tid) = target_id {
            self.raise_event(DuelEvent::new(event_code::BE_BATTLE_TARGET, opp).with_cards(vec![tid]));
        }

        // Check for Replay (if opponent's monster count changed before Damage Step)
        if self.cards_in(opp, Location::MZONE).len() != self.state.battle.expected_opp_monster_count {
            self.state.battle.replay_available = true;
            self.emit(DuelMessage::ReplayAvailable { attacker: attacker_id });
            return Ok(());
        }

        if self.state.battle.attack_negated {
            self.emit(DuelMessage::AttackDisabled { attacker: attacker_id });
            self.state.battle = BattleState::default();
            return Ok(());
        }

        self.resolve_damage_step(host);
        Ok(())
    }

    /// Execute all 5 official Konami Damage Step sub-steps accurately!
    pub fn resolve_damage_step(&mut self, host: &mut dyn ScriptHost) {
        let Some(attacker_id) = self.state.battle.attacker else {
            return;
        };
        let target_id = self.state.battle.defender;

        // 1. Start of the Damage Step
        self.state.phase = Phase::Damage;
        self.state.battle.sub_step = DamageSubStep::StartOfDamageStep;
        self.emit(DuelMessage::DamageSubStep {
            sub_step: DamageSubStep::StartOfDamageStep,
        });
        self.raise_event(DuelEvent::new(event_code::BATTLE_START, self.state.turn_player).with_cards(vec![attacker_id]));

        // 2. Before Damage Calculation — Flip face-down defense target face-up!
        self.state.battle.sub_step = DamageSubStep::BeforeDamageCalculation;
        self.emit(DuelMessage::DamageSubStep {
            sub_step: DamageSubStep::BeforeDamageCalculation,
        });
        if let Some(tid) = target_id {
            if let Some(t) = self.card(tid).cloned() {
                if t.is_facedown() {
                    let new_pos = if t.is_defense_pos() {
                        Position::FACEUP_DEFENSE
                    } else {
                        Position::FACEUP_ATTACK
                    };
                    if let Some(tm) = self.card_mut(tid) {
                        tm.prev_position = t.position;
                        tm.position = new_pos;
                        tm.status.insert(CardStatus::EFFECT_ENABLED);
                    }
                    self.state.battle.flipped_in_damage_step = Some(tid);
                    self.emit(DuelMessage::PosChange {
                        card_id: tid,
                        code: t.data.code,
                        controller: t.controller,
                        location: Location::MZONE,
                        sequence: t.sequence,
                        prev_position: t.position,
                        new_position: new_pos,
                    });
                    self.recalculate_effects(host);
                }
            }
        }
        self.raise_event(DuelEvent::new(event_code::PRE_DAMAGE_CALCULATE, self.state.turn_player).with_cards(vec![attacker_id]));

        // 3. During Damage Calculation
        self.state.phase = Phase::DamageCal;
        self.state.battle.sub_step = DamageSubStep::DuringDamageCalculation;
        self.emit(DuelMessage::DamageSubStep {
            sub_step: DamageSubStep::DuringDamageCalculation,
        });
        self.recalculate_effects(host);

        let Some(atk_card) = self.card(attacker_id).cloned() else {
            return;
        };
        let atk_val = atk_card.current_atk();
        let atk_player = atk_card.controller;
        let opp_player = self.opponent_of(atk_player);

        match target_id {
            None => {
                // Direct Attack!
                self.emit(DuelMessage::Battle {
                    attacker: attacker_id,
                    attacker_atk: atk_val,
                    defender: None,
                    defender_stat: 0,
                });
                if atk_val > 0 {
                    self.inflict_damage(opp_player, atk_val, true, Reason::BATTLE, host);
                }
            }
            Some(def_id) => {
                let Some(def_card) = self.card(def_id).cloned() else {
                    return;
                };
                if def_card.is_attack_pos() {
                    let def_atk = def_card.current_atk();
                    self.emit(DuelMessage::Battle {
                        attacker: attacker_id,
                        attacker_atk: atk_val,
                        defender: Some(def_id),
                        defender_stat: def_atk,
                    });
                    if atk_val > def_atk {
                        let dmg = atk_val - def_atk;
                        self.inflict_damage(def_card.controller, dmg, true, Reason::BATTLE, host);
                        if !def_card.computed.indestructable_battle {
                            self.mark_battle_destroyed(def_id);
                        }
                    } else if atk_val < def_atk {
                        let dmg = def_atk - atk_val;
                        self.inflict_damage(atk_player, dmg, true, Reason::BATTLE, host);
                        if !atk_card.computed.indestructable_battle {
                            self.mark_battle_destroyed(attacker_id);
                        }
                    } else if atk_val > 0 {
                        // Official Rule: Equal ATK > 0 destroys BOTH; 0 ATK vs 0 ATK destroys NEITHER!
                        if !atk_card.computed.indestructable_battle {
                            self.mark_battle_destroyed(attacker_id);
                        }
                        if !def_card.computed.indestructable_battle {
                            self.mark_battle_destroyed(def_id);
                        }
                    }
                } else {
                    // Attacking Defense Position monster!
                    let def_val = def_card.current_def();
                    self.emit(DuelMessage::Battle {
                        attacker: attacker_id,
                        attacker_atk: atk_val,
                        defender: Some(def_id),
                        defender_stat: def_val,
                    });
                    if atk_val > def_val {
                        if !def_card.computed.indestructable_battle {
                            self.mark_battle_destroyed(def_id);
                        }
                        // Official Piercing Battle Damage!
                        if atk_card.computed.pierce {
                            self.inflict_damage(def_card.controller, atk_val - def_val, true, Reason::BATTLE, host);
                        }
                    } else if atk_val < def_val {
                        // Defender not destroyed; attacker takes difference as battle damage!
                        self.inflict_damage(atk_player, def_val - atk_val, true, Reason::BATTLE, host);
                    }
                }
            }
        }

        if self.is_finished() {
            return;
        }

        // 4. After Damage Calculation — FLIP effects activate HERE!
        self.state.phase = Phase::Damage;
        self.state.battle.sub_step = DamageSubStep::AfterDamageCalculation;
        self.emit(DuelMessage::DamageSubStep {
            sub_step: DamageSubStep::AfterDamageCalculation,
        });
        if let Some(flipped_id) = self.state.battle.flipped_in_damage_step {
            let flipped_ctrl = self.card(flipped_id).map(|c| c.controller).unwrap_or(opp_player);
            self.raise_event(DuelEvent::new(event_code::FLIP, flipped_ctrl).with_cards(vec![flipped_id]));
            // Automatically fire mandatory Flip triggers if present
            let flip_triggers = self.collect_segoc_triggers(host);
            for trig in flip_triggers {
                if trig.card_id == flipped_id && trig.mandatory {
                    let _ = self.activate_effect(trig.controller, trig.card_id, trig.effect_id, Vec::new(), host);
                }
            }
            if !self.state.chain.is_empty() {
                self.resolve_chain(host);
            }
        }
        self.raise_event(DuelEvent::new(event_code::BATTLED, atk_player).with_cards(vec![attacker_id]));

        // 5. End of the Damage Step — Send battle-destroyed monsters to GY!
        self.state.battle.sub_step = DamageSubStep::EndOfDamageStep;
        self.emit(DuelMessage::DamageSubStep {
            sub_step: DamageSubStep::EndOfDamageStep,
        });
        let destroyed = std::mem::take(&mut self.state.battle.destroyed_in_battle);
        for did in &destroyed {
            if let Some(dc) = self.card(*did).cloned() {
                let _ = self.move_card(
                    *did,
                    dc.owner,
                    Location::GRAVE,
                    None,
                    Position::FACEUP_ATTACK,
                    Reason::BATTLE | Reason::DESTROY,
                    atk_player,
                    None,
                    host,
                );
            }
        }
        if !destroyed.is_empty() {
            self.raise_event(
                DuelEvent::new(event_code::BATTLE_DESTROYED, atk_player)
                    .with_cards(destroyed)
                    .with_reason(Reason::BATTLE | Reason::DESTROY, atk_player, None),
            );
        }
        self.raise_event(DuelEvent::new(event_code::DAMAGE_STEP_END, atk_player));

        self.state.battle = BattleState::default();
        self.state.phase = Phase::BattleStep;
        self.recalculate_effects(host);
        self.check_win_conditions();
    }

    fn mark_battle_destroyed(&mut self, card_id: CardId) {
        if let Some(c) = self.card_mut(card_id) {
            c.status.insert(CardStatus::BATTLE_DESTROYED);
        }
        if !self.state.battle.destroyed_in_battle.contains(&card_id) {
            self.state.battle.destroyed_in_battle.push(card_id);
        }
    }

    pub fn inflict_damage(
        &mut self,
        player: PlayerId,
        amount: i32,
        is_battle: bool,
        reason: Reason,
        _host: &mut dyn ScriptHost,
    ) {
        if amount <= 0 || self.is_finished() {
            return;
        }
        let team = self.team_of(player);
        let new_lp = (self.state.players[team].lp - amount).max(0);
        self.state.players[team].lp = new_lp;
        if self.state.config.flags.contains(DuelFlags::TAG_MODE) && self.state.players.len() >= 4 {
            self.state.players[team + 2].lp = new_lp;
        }
        // Turbo Duel: lose 1 Speed Counter per 1000 damage taken!
        if self.state.config.flags.contains(DuelFlags::TURBO_DUEL) && amount >= 1000 {
            let loss = (amount / 1000) as u32;
            let sc = &mut self.state.players[team].speed_counters;
            *sc = sc.saturating_sub(loss);
            let val = *sc;
            self.emit(DuelMessage::SpeedCountersUpdated {
                player: team as PlayerId,
                counters: val,
            });
        }
        self.emit(DuelMessage::Damage {
            player: team as PlayerId,
            amount,
            new_lp,
            is_battle,
        });
        let ev = if is_battle { event_code::BATTLE_DAMAGE } else { event_code::DAMAGE };
        self.raise_event(
            DuelEvent::new(ev, team as PlayerId)
                .with_value(amount as i64)
                .with_reason(reason, team as PlayerId, None),
        );
        self.check_win_conditions();
    }

    pub fn recover_lp(
        &mut self,
        player: PlayerId,
        amount: i32,
        reason: Reason,
        _host: &mut dyn ScriptHost,
    ) {
        if amount <= 0 || self.is_finished() {
            return;
        }
        let team = self.team_of(player);
        let new_lp = self.state.players[team].lp + amount;
        self.state.players[team].lp = new_lp;
        if self.state.config.flags.contains(DuelFlags::TAG_MODE) && self.state.players.len() >= 4 {
            self.state.players[team + 2].lp = new_lp;
        }
        self.emit(DuelMessage::Recover {
            player: team as PlayerId,
            amount,
            new_lp,
        });
        self.raise_event(
            DuelEvent::new(event_code::RECOVER, team as PlayerId)
                .with_value(amount as i64)
                .with_reason(reason, team as PlayerId, None),
        );
    }

    pub fn pay_lp_cost(&mut self, player: PlayerId, amount: i32) -> bool {
        let team = self.team_of(player);
        if self.state.players[team].lp <= amount {
            return false;
        }
        let new_lp = self.state.players[team].lp - amount;
        self.state.players[team].lp = new_lp;
        if self.state.config.flags.contains(DuelFlags::TAG_MODE) && self.state.players.len() >= 4 {
            self.state.players[team + 2].lp = new_lp;
        }
        self.emit(DuelMessage::PayLpCost {
            player: team as PlayerId,
            amount,
            new_lp,
        });
        true
    }

    // -------------------------------------------------------------------------
    // WIN CONDITIONS (LP ZERO, RELAY HANDOFF, EXODIA, DECK OUT)
    // -------------------------------------------------------------------------

    pub fn check_win_conditions(&mut self) {
        if self.is_finished() {
            return;
        }
        // 1. Check Exodia the Forbidden One (5 exact pieces in hand!)
        const EXODIA_CODES: [CardCode; 5] = [33396948, 7902349, 70903634, 8124921, 44519536];
        let mut exodia_winners = Vec::new();
        for p in 0..self.state.players.len() as PlayerId {
            let hand_codes: BTreeSet<CardCode> = self
                .cards_in(p, Location::HAND)
                .into_iter()
                .filter_map(|id| self.card(id))
                .map(|c| c.data.effective_alias())
                .collect();
            if EXODIA_CODES.iter().all(|c| hand_codes.contains(c)) {
                exodia_winners.push(self.team_of(p) as PlayerId);
            }
        }
        if exodia_winners.len() == 1 {
            self.declare_win(Some(exodia_winners[0]), WinReason::Exodia);
            return;
        } else if exodia_winners.len() > 1 {
            self.declare_win(None, WinReason::DrawGame);
            return;
        }

        // 2. Check LP == 0 (with Relay Duel teammate handoff!)
        let lp0 = self.state.players[0].lp;
        let lp1 = self.state.players[1].lp;
        if lp0 <= 0 && lp1 <= 0 {
            self.declare_win(None, WinReason::DrawGame);
        } else if lp0 <= 0 {
            if !self.try_relay_handoff(0) {
                self.declare_win(Some(1), WinReason::LpZero);
            }
        } else if lp1 <= 0 {
            if !self.try_relay_handoff(1) {
                self.declare_win(Some(0), WinReason::LpZero);
            }
        }
    }

    /// Official Relay Duel handoff: when a team's LP reaches 0 and a reserve teammate remains,
    /// the reserve teammate takes over with fresh LP & deck while KEEPING the current field!
    fn try_relay_handoff(&mut self, team: usize) -> bool {
        if !self.state.config.flags.contains(DuelFlags::RELAY_MODE) {
            return false;
        }
        let next = self.state.players[team].relay_teammates.first().cloned();
        let Some((name, main, extra)) = next else {
            return false;
        };
        self.state.players[team].relay_teammates.remove(0);
        self.state.players[team].name = name;
        self.state.players[team].lp = self.state.config.starting_lp;

        // Clear old player's Hand & Deck (field stays intact!)
        let to_remove: Vec<CardId> = self
            .state
            .cards
            .values()
            .filter(|c| self.team_of(c.owner) == team && matches!(c.location, Location::DECK | Location::HAND | Location::EXTRA))
            .map(|c| c.id)
            .collect();
        for cid in to_remove {
            self.state.cards.remove(&cid);
        }
        let mut noop = NoopScriptHost;
        self.load_deck(team as PlayerId, main, extra, None, &mut noop);
        self.shuffle_deck(team as PlayerId);
        let sh = self.state.config.starting_hand;
        self.draw_cards(team as PlayerId, sh, Reason::RULE, &mut noop);
        self.emit(DuelMessage::LpUpdate {
            player: team as PlayerId,
            new_lp: self.state.config.starting_lp,
        });
        true
    }

    pub fn declare_win(&mut self, winner: Option<PlayerId>, reason: WinReason) {
        if self.is_finished() {
            return;
        }
        self.state.winner = Some(winner);
        self.state.win_reason = Some(reason);
        self.emit(DuelMessage::Win { winner, reason });
    }

    // -------------------------------------------------------------------------
    // PROMPT GENERATION & DECISION EXECUTION (HIGH-LEVEL INTERACTIVE LOOP)
    // -------------------------------------------------------------------------

    /// Build the current interactive `Prompt` for the active player (respecting Omega Chain Control!).
    pub fn current_prompt(&mut self, host: &mut dyn ScriptHost) -> Option<Prompt> {
        if self.is_finished() {
            return None;
        }
        let tp = self.state.turn_player;
        if self.state.phase.is_main_phase() {
            let hand = self.cards_in(tp, Location::HAND);
            let mzone = self.cards_in(tp, Location::MZONE);
            let mut summonable = Vec::new();
            let mut msetable = Vec::new();
            let mut ssetable = Vec::new();
            let mut reposable = Vec::new();

            for &cid in &hand {
                if self.can_normal_summon(tp, cid) {
                    summonable.push(cid);
                    msetable.push(cid);
                }
                if let Some(c) = self.card(cid) {
                    if (c.data.is_spell() || c.data.is_trap())
                        && (c.data.card_type.contains(CardType::FIELD) || !self.free_stz_zones(tp).is_empty())
                    {
                        ssetable.push(cid);
                    }
                }
            }
            for &cid in &mzone {
                if self.can_normal_summon(tp, cid) {
                    summonable.push(cid); // Gemini second summon
                }
                if let Some(c) = self.card(cid) {
                    if !c.data.is_link()
                        && !c.status.intersects(
                            CardStatus::SUMMON_TURN
                                | CardStatus::SPSUMMON_TURN
                                | CardStatus::SET_TURN
                                | CardStatus::FLIP_SUMMON_TURN
                                | CardStatus::FORM_CHANGED,
                        )
                        && c.attacks_declared_this_turn == 0
                    {
                        reposable.push(cid);
                    }
                }
            }

            let activatable = self.list_activatable_options(tp, host);
            let pend_ok = !self.state.players[tp as usize].pendulum_summoned_this_turn
                && self.pendulum_scales(tp).is_some()
                && !self.state.config.flags.contains(DuelFlags::NO_PENDULUM_OR_LINK);

            let max_trios = self.find_maximum_trios(tp);

            Some(Prompt::SelectIdleCmd {
                player: tp,
                summonable,
                spsummonable: Vec::new(),
                reposable,
                msetable,
                ssetable,
                activatable,
                pendulum_summonable: pend_ok,
                maximum_summonable: max_trios,
                can_battle_phase: self.can_enter_battle_phase(tp),
                can_end_phase: true,
            })
        } else if self.state.phase.is_battle_phase() {
            let mut attackable = Vec::new();
            for cid in self.cards_in(tp, Location::MZONE) {
                let targets = self.valid_attack_targets(cid);
                if !targets.is_empty() {
                    attackable.push((cid, targets));
                }
            }
            let activatable = self.list_activatable_options(tp, host);
            let can_main2 = !self.state.config.flags.contains(DuelFlags::NO_MAIN_PHASE_2);
            Some(Prompt::SelectBattleCmd {
                player: tp,
                attackable,
                activatable,
                can_main2,
                can_end_phase: true,
            })
        } else {
            None
        }
    }

    pub fn list_activatable_options(
        &mut self,
        player: PlayerId,
        host: &mut dyn ScriptHost,
    ) -> Vec<ActivatableOption> {
        let pairs: Vec<(CardId, EffectId, u32, CardType)> = self
            .state
            .effects
            .values()
            .filter(|e| e.active && e.is_activated())
            .filter_map(|e| {
                let cid = e.handler_card?;
                let c = self.state.cards.get(&cid)?;
                Some((cid, e.id, e.description, c.data.card_type))
            })
            .collect();

        let mut opts = Vec::new();
        for (cid, eid, desc, ctype) in pairs {
            if self.can_activate_effect(player, cid, eid, host) {
                let ss = self.state.effects.get(&eid).map(|e| e.spell_speed(ctype)).unwrap_or(1);
                opts.push(ActivatableOption {
                    card_id: cid,
                    effect_id: eid,
                    description: desc,
                    spell_speed: ss,
                });
            }
        }
        opts
    }

    /// Respects YGO Omega's 4-mode Chain Control (`Auto`, `All`, `Legal`, `Off`)!
    pub fn should_prompt_chain(&mut self, player: PlayerId, host: &mut dyn ScriptHost) -> Option<Prompt> {
        let mode = self.chain_control(player);
        if mode == ChainControlMode::Off {
            return None;
        }
        let options = self.list_activatable_options(player, host);
        if options.is_empty() {
            return None;
        }
        match mode {
            ChainControlMode::Off => None,
            ChainControlMode::All | ChainControlMode::Legal | ChainControlMode::Auto => {
                Some(Prompt::SelectChain {
                    player,
                    options,
                    forced: false,
                    chain_level: self.state.chain.len() as u8,
                })
            }
        }
    }

    fn find_maximum_trios(&self, player: PlayerId) -> Vec<[CardId; 3]> {
        if !self.state.config.is_rush() {
            return Vec::new();
        }
        let max_cards: Vec<CardId> = self
            .cards_in(player, Location::HAND)
            .into_iter()
            .filter(|&id| self.card(id).map(|c| c.data.is_maximum()).unwrap_or(false))
            .collect();
        if max_cards.len() >= 3 {
            vec![[max_cards[0], max_cards[1], max_cards[2]]]
        } else {
            Vec::new()
        }
    }

    /// Apply a player `Decision` and advance the engine state.
    pub fn apply_decision(&mut self, decision: Decision, host: &mut dyn ScriptHost) -> Result<(), DuelError> {
        if self.is_finished() {
            return Err(DuelError::DuelEnded);
        }
        self.save_snapshot();
        self.state.action_counter += 1;
        let tp = self.state.turn_player;

        match decision {
            Decision::NormalSummon { card_id, tributes, zone } => {
                self.normal_summon(tp, card_id, &tributes, zone, false, host)?;
            }
            Decision::SetMonster { card_id, tributes, zone } => {
                self.normal_summon(tp, card_id, &tributes, zone, true, host)?;
            }
            Decision::SetSpellTrap { card_id, zone } => {
                self.set_spell_trap(tp, card_id, zone, host)?;
            }
            Decision::ChangePosition { card_id } => {
                self.change_position_manual(tp, card_id, host)?;
            }
            Decision::SpecialSummon { card_id, materials, zone, position } => {
                let c = self.card(card_id).ok_or(DuelError::CardNotFound(card_id))?.clone();
                if c.data.card_type.contains(CardType::SYNCHRO) {
                    self.synchro_summon(tp, card_id, &materials, zone, host)?;
                } else if c.data.is_xyz() {
                    self.xyz_summon(tp, card_id, &materials, zone, host)?;
                } else if c.data.is_link() {
                    self.link_summon(tp, card_id, &materials, zone, host)?;
                } else if c.data.card_type.contains(CardType::FUSION) {
                    self.fusion_summon(tp, card_id, &materials, zone, host)?;
                } else if c.data.is_ritual() {
                    self.ritual_summon(tp, card_id, &materials, zone, host)?;
                } else {
                    self.special_summon(
                        tp,
                        card_id,
                        SummonType::SPECIAL,
                        position.unwrap_or(Position::FACEUP_ATTACK),
                        zone,
                        false,
                        true,
                        host,
                    )?;
                }
            }
            Decision::PendulumSummon { cards } => {
                self.pendulum_summon(tp, &cards, host)?;
            }
            Decision::MaximumSummon { pieces } => {
                self.maximum_summon(tp, pieces, host)?;
            }
            Decision::ActivateEffect { card_id, effect_id, targets }
            | Decision::ChainActivate { card_id, effect_id, targets } => {
                let ctrl = self.card(card_id).map(|c| c.controller).unwrap_or(tp);
                self.activate_effect(ctrl, card_id, effect_id, targets, host)?;
                self.resolve_chain(host);
            }
            Decision::EnterBattlePhase => {
                if !self.can_enter_battle_phase(tp) {
                    return Err(DuelError::IllegalAction("cannot enter Battle Phase now".into()));
                }
                self.advance_phase(Phase::BattleStart, host);
            }
            Decision::DeclareAttack { attacker, target } => {
                self.declare_attack(attacker, target, host)?;
            }
            Decision::EnterMainPhase2 => {
                if self.state.config.flags.contains(DuelFlags::NO_MAIN_PHASE_2) {
                    return Err(DuelError::IllegalAction("no Main Phase 2 in this format".into()));
                }
                if !self.state.phase.is_battle_phase() {
                    return Err(DuelError::IllegalAction("not in Battle Phase".into()));
                }
                self.advance_phase(Phase::BattleEnd, host);
                self.advance_phase(Phase::Main2, host);
            }
            Decision::EndTurn => {
                self.end_turn(host);
            }
            Decision::Pass => {
                if !self.state.chain.is_empty() {
                    self.resolve_chain(host);
                }
            }
            Decision::Surrender(p) => {
                let opp = self.opponent_of(p);
                self.declare_win(Some(opp), WinReason::Surrender);
            }
            _ => {}
        }
        Ok(())
    }

    // -------------------------------------------------------------------------
    // MANUAL MODE & JUDGE / SANDBOX CONTROLS (1:1 YGO OMEGA)
    // -------------------------------------------------------------------------

    pub fn apply_manual_command(
        &mut self,
        cmd: ManualCommand,
        host: &mut dyn ScriptHost,
    ) -> Result<(), DuelError> {
        if let ManualCommand::SetManualMode(enabled) = cmd {
            self.state.manual_mode = enabled;
            self.emit(DuelMessage::ManualAction {
                description: format!("Manual Mode {}", if enabled { "ENABLED" } else { "DISABLED" }),
            });
            return Ok(());
        }
        if !self.state.manual_mode {
            return Err(DuelError::ManualModeDisabled);
        }
        if matches!(cmd, ManualCommand::Undo) {
            if let Some(prev) = self.snapshots.pop() {
                self.state = prev;
                self.emit(DuelMessage::ManualAction {
                    description: "Rolled back duel state to previous snapshot".into(),
                });
                return Ok(());
            }
            return Err(DuelError::IllegalAction("no snapshots available to undo".into()));
        }

        self.save_snapshot();
        match cmd {
            ManualCommand::MoveCard {
                card_id,
                to_controller,
                to_location,
                to_sequence,
                to_position,
            } => {
                let seq = to_sequence.unwrap_or_else(|| self.next_sequence_for(to_controller, to_location));
                self.move_card_raw(
                    card_id,
                    to_controller,
                    to_location,
                    seq,
                    to_position,
                    Reason::RULE,
                    to_controller,
                    None,
                    host,
                );
                self.recalculate_effects(host);
                self.emit(DuelMessage::ManualAction {
                    description: format!("Moved {:?} to {:?} seq {}", card_id, to_location, seq),
                });
            }
            ManualCommand::SetPosition { card_id, position } => {
                if let Some(c) = self.card_mut(card_id) {
                    let prev = c.position;
                    c.prev_position = prev;
                    c.position = position;
                    let code = c.data.code;
                    let ctrl = c.controller;
                    let loc = c.location;
                    let seq = c.sequence;
                    self.emit(DuelMessage::PosChange {
                        card_id,
                        code,
                        controller: ctrl,
                        location: loc,
                        sequence: seq,
                        prev_position: prev,
                        new_position: position,
                    });
                }
                self.recalculate_effects(host);
            }
            ManualCommand::SetLp { player, lp } => self.set_lp(player, lp),
            ManualCommand::AdjustLp { player, delta } => {
                let cur = self.get_lp(player);
                self.set_lp(player, cur + delta);
            }
            ManualCommand::SetAtkOverride { card_id, atk } => {
                if let Some(c) = self.card_mut(card_id) {
                    c.manual_atk_override = atk;
                }
            }
            ManualCommand::SetDefOverride { card_id, def } => {
                if let Some(c) = self.card_mut(card_id) {
                    c.manual_def_override = def;
                }
            }
            ManualCommand::AddCounter { card_id, counter_type, count } => {
                if let Some(c) = self.card_mut(card_id) {
                    c.add_counter(counter_type, count);
                }
                self.emit(DuelMessage::AddCounter {
                    card_id,
                    counter_type,
                    count,
                });
            }
            ManualCommand::RemoveCounter { card_id, counter_type, count } => {
                if let Some(c) = self.card_mut(card_id) {
                    c.remove_counter(counter_type, count);
                }
                self.emit(DuelMessage::RemoveCounter {
                    card_id,
                    counter_type,
                    count,
                });
            }
            ManualCommand::DrawCards { player, count } => {
                self.draw_cards(player, count, Reason::RULE, host);
            }
            ManualCommand::ShuffleDeck { player } => self.shuffle_deck(player),
            ManualCommand::SpawnToken { controller, mut data, position, sequence } => {
                data.card_type.insert(CardType::MONSTER | CardType::TOKEN);
                let seq = sequence.unwrap_or_else(|| self.next_sequence_for(controller, Location::MZONE));
                self.add_card_at(data, controller, controller, Location::MZONE, seq, position, true, host);
                self.recalculate_effects(host);
            }
            ManualCommand::AttachOverlay { material, xyz_monster } => {
                let idx = self.card(xyz_monster).map(|c| c.overlay_materials.len() as u8).unwrap_or(0);
                if let Some(m) = self.card_mut(material) {
                    m.location = Location::OVERLAY;
                    m.sequence = idx;
                    m.overlay_parent = Some(xyz_monster);
                }
                if let Some(x) = self.card_mut(xyz_monster) {
                    x.overlay_materials.push(material);
                }
            }
            ManualCommand::DetachOverlay { xyz_monster, material } => {
                if let Some(x) = self.card_mut(xyz_monster) {
                    x.overlay_materials.retain(|&id| id != material);
                }
                if let Some(m) = self.card_mut(material) {
                    m.overlay_parent = None;
                }
                let owner = self.card(material).map(|c| c.owner).unwrap_or(0);
                let gy_seq = self.cards_in(owner, Location::GRAVE).len() as u8;
                self.move_card_raw(material, owner, Location::GRAVE, gy_seq, Position::FACEUP_ATTACK, Reason::RULE, owner, None, host);
            }
            ManualCommand::TossCoin { player, count } => {
                let results = (0..count).map(|_| self.state.rng.toss_coin()).collect();
                self.emit(DuelMessage::TossCoin { player, results });
            }
            ManualCommand::RollDice { player, count } => {
                let results = (0..count).map(|_| self.state.rng.roll_dice()).collect();
                self.emit(DuelMessage::TossDice { player, results });
            }
            ManualCommand::SetPhase(phase) => self.advance_phase(phase, host),
            ManualCommand::ForceNextTurn => self.end_turn(host),
            ManualCommand::DeclareWinner { winner, reason } => self.declare_win(winner, reason),
            ManualCommand::SetManualMode(_) | ManualCommand::Undo => {}
        }
        Ok(())
    }
}

/// Exact Link Rating subset-sum solver: each material can contribute `1` OR its own Link Rating!
pub fn can_match_link_rating(material_ratings: &[u32], target: u32) -> bool {
    let mut possible = BTreeSet::new();
    possible.insert(0u32);
    for &r in material_ratings {
        let mut next = BTreeSet::new();
        for &cur in &possible {
            if cur + 1 <= target {
                next.insert(cur + 1);
            }
            if r > 1 && cur + r <= target {
                next.insert(cur + r);
            }
        }
        possible = next;
    }
    possible.contains(&target)
}
