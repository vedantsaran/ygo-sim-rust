use serde::{Deserialize, Serialize};
use crate::card::CardId;
use crate::constants::*;
use crate::effect::EffectId;

/// An activatable effect option presented to a player.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivatableOption {
    pub card_id: CardId,
    pub effect_id: EffectId,
    pub description: u32,
    pub spell_speed: u8,
}

/// Interactive prompt emitted by `DuelEngine` when a player must make a decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Prompt {
    SelectIdleCmd {
        player: PlayerId,
        summonable: Vec<CardId>,
        spsummonable: Vec<CardId>,
        reposable: Vec<CardId>,
        msetable: Vec<CardId>,
        ssetable: Vec<CardId>,
        activatable: Vec<ActivatableOption>,
        pendulum_summonable: bool,
        maximum_summonable: Vec<[CardId; 3]>,
        can_battle_phase: bool,
        can_end_phase: bool,
    },
    SelectBattleCmd {
        player: PlayerId,
        attackable: Vec<(CardId, Vec<Option<CardId>>)>,
        activatable: Vec<ActivatableOption>,
        can_main2: bool,
        can_end_phase: bool,
    },
    SelectChain {
        player: PlayerId,
        options: Vec<ActivatableOption>,
        forced: bool,
        chain_level: u8,
    },
    SelectEffectYn {
        player: PlayerId,
        card_id: CardId,
        effect_id: EffectId,
        description: u32,
    },
    SelectYesNo {
        player: PlayerId,
        message: String,
    },
    SelectCard {
        player: PlayerId,
        candidates: Vec<CardId>,
        min: usize,
        max: usize,
        cancelable: bool,
        hint: String,
    },
    SelectTribute {
        player: PlayerId,
        candidates: Vec<CardId>,
        min: usize,
        max: usize,
    },
    SelectPosition {
        player: PlayerId,
        card_id: CardId,
        allowed: Position,
    },
    SelectPlace {
        player: PlayerId,
        location: Location,
        allowed_seqs: Vec<u8>,
    },
    SelectOption {
        player: PlayerId,
        options: Vec<String>,
    },
    AnnounceAttribute {
        player: PlayerId,
        count: usize,
        allowed: Attribute,
    },
    AnnounceRace {
        player: PlayerId,
        count: usize,
        allowed: Race,
    },
    AnnounceNumber {
        player: PlayerId,
        numbers: Vec<i32>,
    },
    RpsChoice {
        player: PlayerId,
    },
    TurnOrderChoice {
        player: PlayerId,
    },
}

impl Prompt {
    pub fn player(&self) -> PlayerId {
        match self {
            Prompt::SelectIdleCmd { player, .. }
            | Prompt::SelectBattleCmd { player, .. }
            | Prompt::SelectChain { player, .. }
            | Prompt::SelectEffectYn { player, .. }
            | Prompt::SelectYesNo { player, .. }
            | Prompt::SelectCard { player, .. }
            | Prompt::SelectTribute { player, .. }
            | Prompt::SelectPosition { player, .. }
            | Prompt::SelectPlace { player, .. }
            | Prompt::SelectOption { player, .. }
            | Prompt::AnnounceAttribute { player, .. }
            | Prompt::AnnounceRace { player, .. }
            | Prompt::AnnounceNumber { player, .. }
            | Prompt::RpsChoice { player }
            | Prompt::TurnOrderChoice { player } => *player,
        }
    }
}

/// Typed response submitted by a player (or AI / Replay / Network peer) to a `Prompt`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Decision {
    // Idle commands
    NormalSummon { card_id: CardId, tributes: Vec<CardId>, zone: Option<u8> },
    SpecialSummon { card_id: CardId, materials: Vec<CardId>, zone: Option<u8>, position: Option<Position> },
    PendulumSummon { cards: Vec<CardId> },
    MaximumSummon { pieces: [CardId; 3] },
    ChangePosition { card_id: CardId },
    SetMonster { card_id: CardId, tributes: Vec<CardId>, zone: Option<u8> },
    SetSpellTrap { card_id: CardId, zone: Option<u8> },
    ActivateEffect { card_id: CardId, effect_id: EffectId, targets: Vec<CardId> },
    EnterBattlePhase,
    EnterMainPhase2,
    EndTurn,

    // Battle commands
    DeclareAttack { attacker: CardId, target: Option<CardId> },

    // Chain & Prompts
    Pass,
    ChainActivate { card_id: CardId, effect_id: EffectId, targets: Vec<CardId> },
    YesNo(bool),
    SelectCards(Vec<CardId>),
    SelectPosition(Position),
    SelectPlace(u8),
    SelectOption(usize),
    AnnounceAttribute(Attribute),
    AnnounceRace(Race),
    AnnounceNumber(i32),
    Rps(RpsMove),
    GoFirst(bool),
    Surrender(PlayerId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RpsMove {
    Rock,
    Paper,
    Scissors,
}

impl RpsMove {
    /// Returns `Some(0)` if `a` wins, `Some(1)` if `b` wins, `None` on tie.
    pub fn winner(a: RpsMove, b: RpsMove) -> Option<PlayerId> {
        use RpsMove::*;
        match (a, b) {
            (x, y) if x == y => None,
            (Rock, Scissors) | (Paper, Rock) | (Scissors, Paper) => Some(0),
            _ => Some(1),
        }
    }
}
