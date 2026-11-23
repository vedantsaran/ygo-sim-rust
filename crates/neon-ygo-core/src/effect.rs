use serde::{Deserialize, Serialize};
use crate::card::CardId;
use crate::constants::*;

pub type EffectId = u32;

/// Callback handle for effect condition / cost / target / operation / value functions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum EffectCallback {
    #[default]
    None,
    /// Built-in Rust callback identified by a well-known string key (for native cards & tests).
    Builtin(String),
    /// Pure-Rust Lua VM function reference ID (managed by `neon-ygo-lua`).
    LuaFunc(u32),
}

impl EffectCallback {
    pub fn is_some(&self) -> bool {
        !matches!(self, EffectCallback::None)
    }
}

/// Static or dynamic value carried by an `Effect` (e.g., `+500` ATK, `LOCATION_REMOVED`, immunity filter, etc.).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EffectValue {
    None,
    Int(i64),
    Callback(EffectCallback),
}

impl Default for EffectValue {
    fn default() -> Self {
        EffectValue::None
    }
}

/// Optional object stored in `Effect.SetLabelObject`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LabelObject {
    Card(CardId),
    Group(Vec<CardId>),
    Effect(EffectId),
    Int(i64),
    Str(String),
}

/// Usage limit semantics matching ocgcore `Effect.SetCountLimit`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CountLimit {
    /// Maximum uses per turn (or per duel if `once_per_duel`).
    pub max_count: u32,
    /// Optional shared Hard-OPT code (`0` means Soft OPT per card instance).
    pub code: u32,
    /// `EFFECT_COUNT_CODE_OATH` — only counts if activation was NOT negated!
    pub is_oath: bool,
    /// `EFFECT_COUNT_CODE_DUEL` — once per duel.
    pub is_duel: bool,
    /// `EFFECT_COUNT_CODE_CHAIN` — once per chain.
    pub is_chain: bool,
    /// Uses recorded this turn on this specific effect instance (for Soft OPT).
    pub used_this_turn: u32,
    /// Uses recorded in current chain (for Once Per Chain).
    pub used_this_chain: u32,
}

impl CountLimit {
    pub fn soft(max_count: u32) -> Self {
        Self {
            max_count,
            code: 0,
            is_oath: false,
            is_duel: false,
            is_chain: false,
            used_this_turn: 0,
            used_this_chain: 0,
        }
    }

    pub fn hard_opt(code: u32) -> Self {
        Self {
            max_count: 1,
            code,
            is_oath: false,
            is_duel: false,
            is_chain: false,
            used_this_turn: 0,
            used_this_chain: 0,
        }
    }

    pub fn hard_oath(code: u32) -> Self {
        Self {
            max_count: 1,
            code,
            is_oath: true,
            is_duel: false,
            is_chain: false,
            used_this_turn: 0,
            used_this_chain: 0,
        }
    }

    pub fn once_per_duel(code: u32) -> Self {
        Self {
            max_count: 1,
            code,
            is_oath: false,
            is_duel: true,
            is_chain: false,
            used_this_turn: 0,
            used_this_chain: 0,
        }
    }
}

/// Full `ocgcore`-compatible Effect object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Effect {
    pub id: EffectId,
    pub owner_card: Option<CardId>,
    pub handler_card: Option<CardId>,
    pub is_player_effect: bool,
    pub owner_player: PlayerId,

    pub effect_type: EffectType,
    pub code: u32,
    pub category: Category,
    pub flags: EffectFlag,
    pub range: Location,
    /// Target range for `EFFECT_TYPE_FIELD` / `EFFECT_TYPE_TARGET`: `(self_range, opp_range)`.
    pub target_range: (u32, u32),
    pub reset_flags: ResetFlag,
    pub reset_count: u8,
    pub reset_phase: Option<Phase>,

    pub count_limit: Option<CountLimit>,
    pub description: u32,
    pub hint_timing: (u32, u32),
    pub label: i64,
    pub label_object: Option<LabelObject>,

    pub value: EffectValue,
    pub condition: EffectCallback,
    pub cost: EffectCallback,
    pub target: EffectCallback,
    pub operation: EffectCallback,

    /// Monotonically increasing registration timestamp (for continuous effect precedence!).
    pub creation_order: u64,
    pub active: bool,
}

impl Effect {
    pub fn new(id: EffectId, owner_card: Option<CardId>, owner_player: PlayerId, creation_order: u64) -> Self {
        Self {
            id,
            owner_card,
            handler_card: owner_card,
            is_player_effect: false,
            owner_player,
            effect_type: EffectType::empty(),
            code: 0,
            category: Category::empty(),
            flags: EffectFlag::empty(),
            range: Location::NONE,
            target_range: (0, 0),
            reset_flags: ResetFlag::empty(),
            reset_count: 0,
            reset_phase: None,
            count_limit: None,
            description: 0,
            hint_timing: (0, 0),
            label: 0,
            label_object: None,
            value: EffectValue::None,
            condition: EffectCallback::None,
            cost: EffectCallback::None,
            target: EffectCallback::None,
            operation: EffectCallback::None,
            creation_order,
            active: true,
        }
    }

    pub fn is_activated(&self) -> bool {
        self.effect_type.intersects(
            EffectType::ACTIVATE
                | EffectType::IGNITION
                | EffectType::TRIGGER_O
                | EffectType::TRIGGER_F
                | EffectType::QUICK_O
                | EffectType::QUICK_F
                | EffectType::FLIP,
        )
    }

    pub fn is_mandatory_trigger(&self) -> bool {
        self.effect_type.intersects(EffectType::TRIGGER_F | EffectType::QUICK_F)
            || (self.effect_type.contains(EffectType::FLIP) && !self.effect_type.contains(EffectType::TRIGGER_O))
    }

    pub fn is_optional_trigger(&self) -> bool {
        self.effect_type.contains(EffectType::TRIGGER_O)
    }

    pub fn is_quick_effect(&self) -> bool {
        self.effect_type.intersects(EffectType::QUICK_O | EffectType::QUICK_F)
    }

    /// True if this optional trigger uses "When... you can" timing (can miss timing!)
    /// vs "If... you can" (`EFFECT_FLAG_DELAY`, never misses timing!).
    pub fn can_miss_timing(&self) -> bool {
        if !self.is_optional_trigger() {
            return false;
        }
        if self.flags.contains(EffectFlag::CAN_MISS_TIMING) {
            return true;
        }
        !self.flags.contains(EffectFlag::DELAY)
    }

    /// Exact official Konami Spell Speed (1, 2, or 3).
    pub fn spell_speed(&self, handler_type: CardType) -> u8 {
        if handler_type.contains(CardType::TRAP) && handler_type.contains(CardType::COUNTER) {
            return 3;
        }
        if self.effect_type.intersects(EffectType::QUICK_O | EffectType::QUICK_F) {
            return 2;
        }
        if self.effect_type.contains(EffectType::ACTIVATE) {
            if handler_type.contains(CardType::TRAP) {
                return 2;
            }
            if handler_type.contains(CardType::SPELL) && handler_type.contains(CardType::QUICKPLAY) {
                return 2;
            }
        }
        1
    }
}
