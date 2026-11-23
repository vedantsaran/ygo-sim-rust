use serde::{Deserialize, Serialize};
use crate::card::CardId;
use crate::constants::*;

/// An atomic game event raised by the engine (used for Trigger evaluation, SEGOC, and Missed Timing).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuelEvent {
    pub code: u32,
    pub cards: Vec<CardId>,
    pub player: PlayerId,
    pub value: i64,
    pub reason: Reason,
    pub reason_player: PlayerId,
    pub reason_effect: Option<u32>,
    /// True ONLY if this event was the very last thing to happen in Chain Link 1 (or open state).
    /// "When... you can" optional triggers require `is_last_thing_to_happen == true`;
    /// "If... you can" (`EFFECT_FLAG_DELAY`) optional triggers and Mandatory triggers ignore this!
    pub is_last_thing_to_happen: bool,
}

impl DuelEvent {
    pub fn new(code: u32, player: PlayerId) -> Self {
        Self {
            code,
            cards: Vec::new(),
            player,
            value: 0,
            reason: Reason::empty(),
            reason_player: player,
            reason_effect: None,
            is_last_thing_to_happen: true,
        }
    }

    pub fn with_cards(mut self, cards: Vec<CardId>) -> Self {
        self.cards = cards;
        self
    }

    pub fn with_reason(mut self, reason: Reason, reason_player: PlayerId, reason_effect: Option<u32>) -> Self {
        self.reason = reason;
        self.reason_player = reason_player;
        self.reason_effect = reason_effect;
        self
    }

    pub fn with_value(mut self, value: i64) -> Self {
        self.value = value;
        self
    }
}

/// Complete YGOPro / EDOPro / YGO Omega duel event message stream (1:1 with `MSG_*`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DuelMessage {
    Start {
        first_player: PlayerId,
        lp: [i32; 2],
        deck_counts: [usize; 2],
        extra_counts: [usize; 2],
    },
    NewTurn {
        turn: u32,
        player: PlayerId,
    },
    NewPhase {
        phase: Phase,
    },
    DamageSubStep {
        sub_step: DamageSubStep,
    },
    Draw {
        player: PlayerId,
        cards: Vec<CardId>,
    },
    Move {
        card_id: CardId,
        code: CardCode,
        from_controller: PlayerId,
        from_location: Location,
        from_sequence: u8,
        to_controller: PlayerId,
        to_location: Location,
        to_sequence: u8,
        to_position: Position,
        reason: Reason,
    },
    PosChange {
        card_id: CardId,
        code: CardCode,
        controller: PlayerId,
        location: Location,
        sequence: u8,
        prev_position: Position,
        new_position: Position,
    },
    Set {
        card_id: CardId,
        controller: PlayerId,
        location: Location,
        sequence: u8,
        position: Position,
    },
    Summoning {
        card_id: CardId,
        code: CardCode,
        controller: PlayerId,
        sequence: u8,
        position: Position,
        summon_type: SummonType,
    },
    Summoned {
        card_id: CardId,
        summon_type: SummonType,
    },
    SpSummoning {
        card_id: CardId,
        code: CardCode,
        controller: PlayerId,
        sequence: u8,
        position: Position,
        summon_type: SummonType,
    },
    SpSummoned {
        card_id: CardId,
        summon_type: SummonType,
    },
    FlipSummoning {
        card_id: CardId,
        code: CardCode,
        controller: PlayerId,
        sequence: u8,
    },
    FlipSummoned {
        card_id: CardId,
    },
    Chaining {
        chain_link: u8,
        card_id: CardId,
        code: CardCode,
        controller: PlayerId,
        location: Location,
        sequence: u8,
        desc: u32,
    },
    Chained {
        chain_link: u8,
    },
    ChainSolving {
        chain_link: u8,
    },
    ChainSolved {
        chain_link: u8,
    },
    ChainEnd,
    ChainNegated {
        chain_link: u8,
    },
    ChainDisabled {
        chain_link: u8,
    },
    Damage {
        player: PlayerId,
        amount: i32,
        new_lp: i32,
        is_battle: bool,
    },
    Recover {
        player: PlayerId,
        amount: i32,
        new_lp: i32,
    },
    LpUpdate {
        player: PlayerId,
        new_lp: i32,
    },
    PayLpCost {
        player: PlayerId,
        amount: i32,
        new_lp: i32,
    },
    Attack {
        attacker: CardId,
        target: Option<CardId>,
    },
    Battle {
        attacker: CardId,
        attacker_atk: i32,
        defender: Option<CardId>,
        defender_stat: i32,
    },
    AttackDisabled {
        attacker: CardId,
    },
    ReplayAvailable {
        attacker: CardId,
    },
    ConfirmCards {
        player: PlayerId,
        cards: Vec<CardId>,
    },
    ShuffleDeck {
        player: PlayerId,
    },
    ShuffleHand {
        player: PlayerId,
    },
    TossCoin {
        player: PlayerId,
        results: Vec<u8>,
    },
    TossDice {
        player: PlayerId,
        results: Vec<u8>,
    },
    AddCounter {
        card_id: CardId,
        counter_type: u16,
        count: u32,
    },
    RemoveCounter {
        card_id: CardId,
        counter_type: u16,
        count: u32,
    },
    Equip {
        equip_card: CardId,
        target_card: CardId,
    },
    SpeedCountersUpdated {
        player: PlayerId,
        counters: u32,
    },
    SkillActivated {
        player: PlayerId,
        card_id: CardId,
        name: String,
    },
    Hint {
        player: PlayerId,
        message: String,
    },
    ManualAction {
        description: String,
    },
    Win {
        winner: Option<PlayerId>,
        reason: WinReason,
    },
}
