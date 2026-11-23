use std::collections::BTreeMap;
use serde::{Deserialize, Serialize};
use crate::constants::*;

/// Unique runtime handle for a specific card instance inside a Duel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CardId(pub u32);

/// Static card definition matching YGOPro/EDOPro/YGO Omega `.cdb` (`datas` + `texts` tables).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardData {
    pub code: CardCode,
    pub alias: CardCode,
    /// Up to 4 archetype setcodes (16-bit each, supporting 12-bit base + 4-bit sub-archetype).
    pub setcodes: Vec<u16>,
    pub card_type: CardType,
    /// Base ATK (`-2` represents `?` in ocgcore).
    pub atk: i32,
    /// Base DEF (`-2` represents `?`; Link monsters use `link_markers`).
    pub def: i32,
    /// Level (Monster), Rank (Xyz), or Link Rating (Link).
    pub level: u32,
    pub race: Race,
    pub attribute: Attribute,
    pub lscale: u32,
    pub rscale: u32,
    pub link_markers: LinkMarker,
    /// OCG/TCG/Rush/Speed scope bitmask (`1=OCG, 2=TCG, 3=OCG/TCG, 4=Custom, 8=Speed, 0x10=Rush`).
    pub ot: u32,
    pub category: Category,
    pub name: String,
    pub desc: String,
    pub strings: Vec<String>,
    /// Rush Duel Maximum ATK (if Maximum Center piece).
    pub maximum_atk: Option<i32>,
    /// Genesys format point weight (0 by default).
    pub genesys_points: u32,
}

impl CardData {
    pub fn effective_alias(&self) -> CardCode {
        if self.alias != 0 { self.alias } else { self.code }
    }

    pub fn is_monster(&self) -> bool {
        self.card_type.contains(CardType::MONSTER)
    }

    pub fn is_spell(&self) -> bool {
        self.card_type.contains(CardType::SPELL)
    }

    pub fn is_trap(&self) -> bool {
        self.card_type.contains(CardType::TRAP)
    }

    pub fn is_extra_deck(&self) -> bool {
        self.card_type.intersects(CardType::EXTRA_DECK)
    }

    pub fn is_link(&self) -> bool {
        self.card_type.contains(CardType::LINK)
    }

    pub fn is_xyz(&self) -> bool {
        self.card_type.contains(CardType::XYZ)
    }

    pub fn is_pendulum(&self) -> bool {
        self.card_type.contains(CardType::PENDULUM)
    }

    pub fn is_tuner(&self) -> bool {
        self.card_type.contains(CardType::TUNER)
    }

    pub fn is_ritual(&self) -> bool {
        self.card_type.contains(CardType::RITUAL)
    }

    pub fn is_maximum(&self) -> bool {
        self.card_type.contains(CardType::MAXIMUM)
    }

    pub fn is_skill(&self) -> bool {
        self.card_type.contains(CardType::SKILL)
    }

    pub fn is_legend(&self) -> bool {
        self.card_type.contains(CardType::LEGEND)
    }

    /// Official ocgcore setcode check (matches 12-bit base archetype OR exact 16-bit sub-archetype).
    pub fn is_set_card(&self, setcode: u16) -> bool {
        let base = setcode & 0x0FFF;
        let sub = setcode & 0xF000;
        for &sc in &self.setcodes {
            if sc == 0 {
                continue;
            }
            if (sc & 0x0FFF) == base && (sub == 0 || (sc & 0xF000) == sub) {
                return true;
            }
        }
        false
    }

    /// Parse 64-bit packed setcode from `.cdb` into up to 4 u16 entries.
    pub fn unpack_setcode(packed: u64) -> Vec<u16> {
        let mut out = Vec::new();
        for shift in [0, 16, 32, 48] {
            let v = ((packed >> shift) & 0xFFFF) as u16;
            if v != 0 {
                out.push(v);
            }
        }
        out
    }

    /// Required tribute count for Normal Summoning this monster.
    pub fn required_tributes(&self) -> u8 {
        if !self.is_monster() {
            return 0;
        }
        if self.level >= 7 {
            2
        } else if self.level >= 5 {
            1
        } else {
            0
        }
    }
}

/// Live effective properties after continuous effects & modifiers are applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputedStats {
    pub code: CardCode,
    pub extra_codes: Vec<CardCode>,
    pub card_type: CardType,
    pub base_atk: i32,
    pub base_def: i32,
    pub atk: i32,
    pub def: i32,
    pub level: u32,
    pub rank: u32,
    pub link: u32,
    pub lscale: u32,
    pub rscale: u32,
    pub race: Race,
    pub attribute: Attribute,
    pub negated: bool,
    pub indestructable_battle: bool,
    pub indestructable_effect: bool,
    pub untargetable_effect: bool,
    pub untargetable_battle: bool,
    pub cannot_attack: bool,
    pub direct_attack: bool,
    pub pierce: bool,
    pub extra_attacks: u32,
    pub immune_to_all_effects: bool,
    pub immune_to_spells: bool,
    pub immune_to_traps: bool,
    pub immune_to_monsters: bool,
}

/// A live card instance inside a running duel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardInstance {
    pub id: CardId,
    pub data: CardData,
    pub owner: PlayerId,
    pub controller: PlayerId,
    pub location: Location,
    /// Zone index (MMZ: 0..5 + EMZ 5,6; STZ: 0..5 + Field 5 + PZone 6,7; Deck/GY/Hand/Banished: list index).
    pub sequence: u8,
    pub position: Position,

    // Previous state tracking (essential for "leaves the field", "sent from field to GY", etc.)
    pub prev_controller: PlayerId,
    pub prev_location: Location,
    pub prev_sequence: u8,
    pub prev_position: Position,

    pub status: CardStatus,
    pub summon_type: SummonType,
    pub summon_player: PlayerId,
    pub summon_location: Location,
    pub turn_id: u32,
    pub reason: Reason,
    pub reason_player: PlayerId,
    pub reason_card: Option<CardId>,
    pub reason_effect: Option<u32>,

    /// Attached Xyz Materials (Overlay Units).
    pub overlay_materials: Vec<CardId>,
    /// If this card is currently attached as an Xyz material, its parent Xyz monster.
    pub overlay_parent: Option<CardId>,
    /// Equip Spell's target monster.
    pub equip_target: Option<CardId>,
    /// Equip cards equipped to this monster.
    pub equipped_cards: Vec<CardId>,
    /// Rush Duel Maximum Mode linked pieces (Left, Right).
    pub maximum_pieces: Vec<CardId>,

    /// Counters on this card: `counter_type -> count`.
    pub counters: BTreeMap<u16, u32>,
    /// Number of attacks declared this turn.
    pub attacks_declared_this_turn: u32,
    /// Registered `EffectId`s belonging to this card.
    pub effects: Vec<u32>,
    /// Custom per-card flag effects (`RegisterFlagEffect` in Lua!).
    pub flag_effects: BTreeMap<u32, i64>,
    /// Manual ATK/DEF overrides (used by Manual / Judge mode).
    pub manual_atk_override: Option<i32>,
    pub manual_def_override: Option<i32>,
    /// Live computed stats updated on every continuous effect pass.
    pub computed: ComputedStats,
}

impl CardInstance {
    pub fn new(id: CardId, data: CardData, owner: PlayerId, location: Location, sequence: u8) -> Self {
        let computed = Self::base_computed(&data);
        let position = match location {
            Location::DECK | Location::EXTRA => Position::FACEDOWN_DEFENSE,
            Location::GRAVE | Location::HAND | Location::SKILL => Position::FACEUP_ATTACK,
            _ => Position::FACEUP_ATTACK,
        };
        Self {
            id,
            data,
            owner,
            controller: owner,
            location,
            sequence,
            position,
            prev_controller: owner,
            prev_location: Location::NONE,
            prev_sequence: 0,
            prev_position: Position::NONE,
            status: CardStatus::empty(),
            summon_type: SummonType::empty(),
            summon_player: owner,
            summon_location: Location::NONE,
            turn_id: 0,
            reason: Reason::empty(),
            reason_player: owner,
            reason_card: None,
            reason_effect: None,
            overlay_materials: Vec::new(),
            overlay_parent: None,
            equip_target: None,
            equipped_cards: Vec::new(),
            maximum_pieces: Vec::new(),
            counters: BTreeMap::new(),
            attacks_declared_this_turn: 0,
            effects: Vec::new(),
            flag_effects: BTreeMap::new(),
            manual_atk_override: None,
            manual_def_override: None,
            computed,
        }
    }

    pub fn base_computed(data: &CardData) -> ComputedStats {
        let atk = data.atk.max(0);
        let def = if data.is_link() { 0 } else { data.def.max(0) };
        ComputedStats {
            code: data.effective_alias(),
            extra_codes: Vec::new(),
            card_type: data.card_type,
            base_atk: atk,
            base_def: def,
            atk,
            def,
            level: if data.is_xyz() || data.is_link() { 0 } else { data.level },
            rank: if data.is_xyz() { data.level } else { 0 },
            link: if data.is_link() { data.level } else { 0 },
            lscale: data.lscale,
            rscale: data.rscale,
            race: data.race,
            attribute: data.attribute,
            negated: false,
            indestructable_battle: false,
            indestructable_effect: false,
            untargetable_effect: false,
            untargetable_battle: false,
            cannot_attack: false,
            direct_attack: false,
            pierce: false,
            extra_attacks: 0,
            immune_to_all_effects: false,
            immune_to_spells: false,
            immune_to_traps: false,
            immune_to_monsters: false,
        }
    }

    pub fn is_faceup(&self) -> bool {
        self.position.intersects(Position::FACEUP)
    }

    pub fn is_facedown(&self) -> bool {
        self.position.intersects(Position::FACEDOWN)
    }

    pub fn is_attack_pos(&self) -> bool {
        self.position.intersects(Position::ATTACK)
    }

    pub fn is_defense_pos(&self) -> bool {
        self.position.intersects(Position::DEFENSE)
    }

    pub fn is_on_field(&self) -> bool {
        self.location.intersects(Location::ONFIELD | Location::FZONE | Location::PZONE)
    }

    pub fn is_public(&self) -> bool {
        match self.location {
            Location::GRAVE | Location::SKILL => true,
            Location::REMOVED | Location::EXTRA | Location::MZONE | Location::SZONE | Location::FZONE | Location::PZONE => self.is_faceup(),
            _ => false,
        }
    }

    /// Face-down banished cards have NO publicly queryable name/type/attribute/race!
    pub fn is_facedown_banished(&self) -> bool {
        self.location == Location::REMOVED && self.is_facedown()
    }

    pub fn has_code(&self, code: CardCode) -> bool {
        if self.is_facedown_banished() {
            return false;
        }
        self.computed.code == code
            || self.data.code == code
            || self.data.effective_alias() == code
            || self.computed.extra_codes.contains(&code)
    }

    pub fn current_atk(&self) -> i32 {
        if let Some(m) = self.manual_atk_override {
            return m.max(0);
        }
        if self.status.contains(CardStatus::MAXIMUM_MODE) {
            if let Some(max_atk) = self.data.maximum_atk {
                return max_atk.max(0);
            }
        }
        self.computed.atk.max(0)
    }

    pub fn current_def(&self) -> i32 {
        if let Some(m) = self.manual_def_override {
            return m.max(0);
        }
        if self.data.is_link() {
            return 0;
        }
        self.computed.def.max(0)
    }

    pub fn is_properly_summoned(&self) -> bool {
        self.status.contains(CardStatus::PROC_COMPLETE)
    }

    /// Monsters that require Proper Special Summon before revival from GY/Banished.
    pub fn requires_proper_summon(&self) -> bool {
        self.data.is_extra_deck()
            || self.data.is_ritual()
            || self.data.card_type.contains(CardType::SPSUMMON)
            || self.status.contains(CardStatus::REVIVE_LIMIT)
    }

    pub fn is_disabled(&self) -> bool {
        self.computed.negated || self.status.contains(CardStatus::DISABLED)
    }

    pub fn is_gemini_effect_active(&self) -> bool {
        !self.data.card_type.contains(CardType::GEMINI)
            || self.status.contains(CardStatus::GEMINI_SUMMONED)
    }

    pub fn counter_count(&self, counter_type: u16) -> u32 {
        if counter_type == 0 {
            self.counters.values().sum()
        } else {
            self.counters.get(&counter_type).copied().unwrap_or(0)
        }
    }

    pub fn add_counter(&mut self, counter_type: u16, count: u32) {
        if count > 0 {
            *self.counters.entry(counter_type).or_insert(0) += count;
        }
    }

    pub fn remove_counter(&mut self, counter_type: u16, count: u32) -> bool {
        if let Some(entry) = self.counters.get_mut(&counter_type) {
            if *entry >= count {
                *entry -= count;
                if *entry == 0 {
                    self.counters.remove(&counter_type);
                }
                return true;
            }
        }
        false
    }
}
