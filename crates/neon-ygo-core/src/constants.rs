use bitflags::bitflags;
use serde::{Deserialize, Serialize};

pub type PlayerId = u8;
pub type CardCode = u32;

bitflags! {
    /// Official ocgcore / EDOPro / YGO Omega card locations.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
    pub struct Location: u32 {
        const NONE        = 0x00;
        const DECK        = 0x01;
        const HAND        = 0x02;
        const MZONE       = 0x04;
        const SZONE       = 0x08;
        const GRAVE       = 0x10;
        const REMOVED     = 0x20;
        const EXTRA       = 0x40;
        const OVERLAY     = 0x80;
        const ONFIELD     = Self::MZONE.bits() | Self::SZONE.bits();
        const FZONE       = 0x100;
        const PZONE       = 0x200;
        const SKILL       = 0x400;
        const ALL         = 0x7FF;
    }
}

bitflags! {
    /// Official ocgcore card positions.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
    pub struct Position: u32 {
        const NONE             = 0x0;
        const FACEUP_ATTACK    = 0x1;
        const FACEDOWN_ATTACK  = 0x2;
        const FACEUP_DEFENSE   = 0x4;
        const FACEDOWN_DEFENSE = 0x8;
        const FACEUP           = Self::FACEUP_ATTACK.bits() | Self::FACEUP_DEFENSE.bits();
        const FACEDOWN         = Self::FACEDOWN_ATTACK.bits() | Self::FACEDOWN_DEFENSE.bits();
        const ATTACK           = Self::FACEUP_ATTACK.bits() | Self::FACEDOWN_ATTACK.bits();
        const DEFENSE          = Self::FACEUP_DEFENSE.bits() | Self::FACEDOWN_DEFENSE.bits();
    }
}

bitflags! {
    /// Official ocgcore / EDOPro / Omega card types (32-bit bitmask matching `.cdb` datas.type).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
    pub struct CardType: u32 {
        const MONSTER     = 0x1;
        const SPELL       = 0x2;
        const TRAP        = 0x4;
        const NORMAL      = 0x10;
        const EFFECT      = 0x20;
        const FUSION      = 0x40;
        const RITUAL      = 0x80;
        const TRAPMONSTER = 0x100;
        const SPIRIT      = 0x200;
        const UNION       = 0x400;
        const GEMINI      = 0x800; // TYPE_DUAL
        const TUNER       = 0x1000;
        const SYNCHRO     = 0x2000;
        const TOKEN       = 0x4000;
        const MAXIMUM     = 0x8000; // Rush Duel Maximum Monster
        const QUICKPLAY   = 0x10000;
        const CONTINUOUS  = 0x20000;
        const EQUIP       = 0x40000;
        const FIELD       = 0x80000;
        const COUNTER     = 0x100000;
        const FLIP        = 0x200000;
        const TOON        = 0x400000;
        const XYZ         = 0x800000;
        const PENDULUM    = 0x1000000;
        const SPSUMMON    = 0x2000000;
        const LINK        = 0x4000000;
        const SKILL       = 0x8000000;
        const ACTION      = 0x10000000;
        const LEGEND      = 0x20000000; // Rush Duel Legend Card
        const EXTRA_DECK  = Self::FUSION.bits() | Self::SYNCHRO.bits() | Self::XYZ.bits() | Self::LINK.bits();
    }
}

bitflags! {
    /// Official ocgcore monster attributes.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
    pub struct Attribute: u32 {
        const NONE   = 0x00;
        const EARTH  = 0x01;
        const WATER  = 0x02;
        const FIRE   = 0x04;
        const WIND   = 0x08;
        const LIGHT  = 0x10;
        const DARK   = 0x20;
        const DIVINE = 0x40;
        const ALL    = 0x7F;
    }
}

bitflags! {
    /// Official ocgcore / EDOPro monster races (types), including Illusion & Rush Duel races.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
    pub struct Race: u64 {
        const NONE             = 0x0;
        const WARRIOR          = 0x1;
        const SPELLCASTER      = 0x2;
        const FAIRY            = 0x4;
        const FIEND            = 0x8;
        const ZOMBIE           = 0x10;
        const MACHINE          = 0x20;
        const AQUA             = 0x40;
        const PYRO             = 0x80;
        const ROCK             = 0x100;
        const WINGEDBEAST      = 0x200;
        const PLANT            = 0x400;
        const INSECT           = 0x800;
        const THUNDER          = 0x1000;
        const DRAGON           = 0x2000;
        const BEAST            = 0x4000;
        const BEASTWARRIOR     = 0x8000;
        const DINOSAUR         = 0x10000;
        const FISH             = 0x20000;
        const SEASERPENT       = 0x40000;
        const REPTILE          = 0x80000;
        const PSYCHIC          = 0x100000;
        const DIVINE           = 0x200000;
        const CREATORGOD       = 0x400000;
        const WYRM             = 0x800000;
        const CYBERSE          = 0x1000000;
        const ILLUSION         = 0x2000000;
        // Rush Duel exclusive races
        const CYBORG           = 0x4000000;
        const MAGICALKNIGHT    = 0x8000000;
        const HIGHDRAGON       = 0x10000000;
        const OMEGAPSYCHIC     = 0x20000000;
        const CELESTIALWARRIOR = 0x40000000;
        const GALAXY           = 0x80000000;
    }
}

bitflags! {
    /// Official ocgcore Link Marker directional arrows.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
    pub struct LinkMarker: u32 {
        const BOTTOM_LEFT  = 0x001;
        const BOTTOM       = 0x002;
        const BOTTOM_RIGHT = 0x004;
        const LEFT         = 0x008;
        const RIGHT        = 0x020;
        const TOP_LEFT     = 0x040;
        const TOP          = 0x080;
        const TOP_RIGHT    = 0x100;
    }
}

/// Official Yu-Gi-Oh! Turn Phases matching `PHASE_*` bit values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u32)]
pub enum Phase {
    Draw        = 0x01,
    Standby     = 0x02,
    Main1       = 0x04,
    BattleStart = 0x08,
    BattleStep  = 0x10,
    Damage      = 0x20,
    DamageCal   = 0x40,
    BattleEnd   = 0x80,
    Main2       = 0x100,
    End         = 0x200,
}

impl Phase {
    pub fn is_battle_phase(self) -> bool {
        matches!(
            self,
            Phase::BattleStart
                | Phase::BattleStep
                | Phase::Damage
                | Phase::DamageCal
                | Phase::BattleEnd
        )
    }

    pub fn is_main_phase(self) -> bool {
        matches!(self, Phase::Main1 | Phase::Main2)
    }

    pub fn code(self) -> u32 {
        self as u32
    }

    pub fn from_code(v: u32) -> Option<Self> {
        match v {
            0x01 => Some(Phase::Draw),
            0x02 => Some(Phase::Standby),
            0x04 => Some(Phase::Main1),
            0x08 => Some(Phase::BattleStart),
            0x10 => Some(Phase::BattleStep),
            0x20 => Some(Phase::Damage),
            0x40 => Some(Phase::DamageCal),
            0x80 => Some(Phase::BattleEnd),
            0x100 => Some(Phase::Main2),
            0x200 => Some(Phase::End),
            _ => None,
        }
    }
}

/// Official Konami 5 Sub-Steps of the Damage Step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum DamageSubStep {
    #[default]
    None,
    /// 1. Start of the Damage Step (before flipping face-down monster)
    StartOfDamageStep,
    /// 2. Before Damage Calculation (flip face-down defense monster face-up; ATK/DEF mods)
    BeforeDamageCalculation,
    /// 3. During Damage Calculation (calculate battle damage, determine battle destruction)
    DuringDamageCalculation,
    /// 4. After Damage Calculation (Flip effects & battle damage triggers activate here!)
    AfterDamageCalculation,
    /// 5. End of the Damage Step (send battle-destroyed monsters to GY; "destroyed by battle" triggers)
    EndOfDamageStep,
}

bitflags! {
    /// Official ocgcore `EFFECT_TYPE_*` flags.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
    pub struct EffectType: u32 {
        const SINGLE       = 0x0001;
        const FIELD        = 0x0002;
        const EQUIP        = 0x0004;
        const ACTIONS      = 0x0008;
        const ACTIVATE     = 0x0010;
        const FLIP         = 0x0020;
        const IGNITION     = 0x0040;
        const TRIGGER_O    = 0x0080;
        const QUICK_O      = 0x0100;
        const TRIGGER_F    = 0x0200;
        const QUICK_F      = 0x0400;
        const CONTINUOUS   = 0x0800;
        const XMATERIAL    = 0x1000;
        const GRANT        = 0x2000;
        const TARGET       = 0x4000;
        const UNCLASSIFIED = 0x8000;
    }
}

bitflags! {
    /// Official ocgcore `EFFECT_FLAG_*` flags.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
    pub struct EffectFlag: u64 {
        const INITIAL                 = 0x0001;
        const FUNC_VALUE              = 0x0002;
        const COUNT_LIMIT             = 0x0004;
        const REQUIRE_TARGET          = 0x0008;
        const CARD_TARGET             = 0x0010;
        const CHECK_SIZE              = 0x0020;
        const OWNER_RELATE            = 0x0040;
        const PLAYER_TARGET           = 0x0080;
        const TARGET_PLAYER           = 0x0100;
        const ABSOLUTE_TARGET         = 0x0200;
        const IGNORE_IMMUNE           = 0x0400;
        const SET_AVAILABLE           = 0x0800;
        const IGNORE_RANGE            = 0x1000;
        const SINGLE_RANGE            = 0x2000;
        const REPEAT                  = 0x4000;
        const DELAY                   = 0x8000;
        const DAMAGE_STEP             = 0x10000;
        const DAMAGE_CAL              = 0x20000;
        const COPY_INHERIT            = 0x40000;
        const UNCOPYABLE              = 0x80000;
        const CANNOT_DISABLE          = 0x100000;
        const CANNOT_NEGATE           = 0x200000;
        const CANNOT_INACTIVATE       = 0x400000;
        const OATH                    = 0x800000;
        const NO_TURN_RESET           = 0x1000000;
        const CLIENT_HINT             = 0x2000000;
        const BOTH_SIDE               = 0x4000000;
        const SPSUM_PARAM             = 0x8000000;
        const CAN_MISS_TIMING         = 0x10000000; // "When... you can" (non-DELAY optional trigger)
        const HARD_OPT                = 0x20000000; // "only use/activate 1 [Card Name] per turn"
        const ONCE_PER_DUEL           = 0x40000000;
    }
}

bitflags! {
    /// Official ocgcore `CATEGORY_*` flags.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
    pub struct Category: u64 {
        const DESTROY         = 0x00000001;
        const RELEASE         = 0x00000002;
        const REMOVE          = 0x00000004;
        const TOHAND          = 0x00000008;
        const TODECK          = 0x00000010;
        const TOGRAVE         = 0x00000020;
        const DECKDES         = 0x00000040;
        const HANDES          = 0x00000080;
        const SUMMON          = 0x00000100;
        const SPECIAL_SUMMON  = 0x00000200;
        const TOKEN           = 0x00000400;
        const FLIP            = 0x00000800;
        const POSITION        = 0x00001000;
        const CONTROL         = 0x00002000;
        const DISABLE         = 0x00004000;
        const DISABLE_SUMMON  = 0x00008000;
        const DRAW            = 0x00010000;
        const SEARCH          = 0x00020000;
        const EQUIP           = 0x00040000;
        const DAMAGE          = 0x00080000;
        const RECOVER         = 0x00100000;
        const ATKCHANGE       = 0x00200000;
        const DEFCHANGE       = 0x00400000;
        const COUNTER         = 0x00800000;
        const COIN            = 0x01000000;
        const DICE            = 0x02000000;
        const LEAVE_GRAVE     = 0x04000000;
        const LVCHANGE        = 0x08000000;
        const NEGLECT         = 0x10000000;
        const ANNOUNCE        = 0x20000000;
        const FUSION_SUMMON   = 0x40000000;
        const TOEXTRA         = 0x80000000;
        const NEGATED         = 0x100000000;
    }
}

bitflags! {
    /// Official ocgcore `REASON_*` flags.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
    pub struct Reason: u32 {
        const DESTROY     = 0x00001;
        const RELEASE     = 0x00002;
        const TEMPORARY   = 0x00004;
        const MATERIAL    = 0x00008;
        const SUMMON      = 0x00010;
        const BATTLE      = 0x00020;
        const EFFECT      = 0x00040;
        const COST        = 0x00080;
        const ADJUST      = 0x00100;
        const LOST_TARGET = 0x00200;
        const RULE        = 0x00400;
        const SPSUMMON    = 0x00800;
        const DISSUMMON   = 0x01000;
        const FLIP        = 0x02000;
        const DISCARD     = 0x04000;
        const RDAMAGE     = 0x08000;
        const RRECOVER    = 0x10000;
        const RETURN      = 0x20000;
        const FUSION      = 0x40000;
        const SYNCHRO     = 0x80000;
        const RITUAL      = 0x100000;
        const XYZ         = 0x200000;
        const REPLACE     = 0x1000000;
        const DRAW        = 0x2000000;
        const REDIRECT    = 0x4000000;
        const LINK        = 0x10000000;
    }
}

bitflags! {
    /// Official ocgcore `SUMMON_TYPE_*` flags.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
    pub struct SummonType: u32 {
        const NORMAL   = 0x10000000;
        const ADVANCE  = 0x11000000; // Tribute Summon
        const GEMINI   = 0x12000000;
        const FLIP     = 0x20000000;
        const SPECIAL  = 0x40000000;
        const FUSION   = 0x43000000;
        const RITUAL   = 0x45000000;
        const SYNCHRO  = 0x46000000;
        const XYZ      = 0x49000000;
        const PENDULUM = 0x4A000000;
        const LINK     = 0x4C000000;
        const MAXIMUM  = 0x4E000000;
    }
}

bitflags! {
    /// Official ocgcore `STATUS_*` runtime card flags.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
    pub struct CardStatus: u32 {
        const DISABLED             = 0x00000001;
        const TO_ENABLE            = 0x00000002;
        const TO_DISABLE           = 0x00000004;
        const PROC_COMPLETE        = 0x00000008; // Properly Special Summoned!
        const SET_TURN             = 0x00000010;
        const NO_LEVEL             = 0x00000020;
        const BATTLE_RESULT        = 0x00000040;
        const SPSUMMON_STEP        = 0x00000080;
        const FORM_CHANGED         = 0x00000100;
        const SUMMONING            = 0x00000200;
        const EFFECT_ENABLED       = 0x00000400;
        const SUMMON_TURN          = 0x00000800;
        const DESTROY_CONFIRMED    = 0x00001000;
        const LEAVE_CONFIRMED      = 0x00002000;
        const BATTLE_DESTROYED     = 0x00004000;
        const COPY_EFFECT          = 0x00008000;
        const CHAINING             = 0x00010000;
        const REVIVE_LIMIT         = 0x00020000;
        const ATTACK_CANCELED      = 0x00080000;
        const INITIALIZING         = 0x00100000;
        const ACTIVATED            = 0x00200000;
        const JUST_POS             = 0x00400000;
        const CONTINUOUS_POS       = 0x00800000;
        const FORBIDDEN            = 0x01000000;
        const ACTIVATE_DISABLED    = 0x02000000;
        const SUMMON_DISABLED      = 0x04000000;
        const SPSUMMON_TURN        = 0x08000000;
        const FLIP_SUMMON_TURN     = 0x10000000;
        const GEMINI_SUMMONED      = 0x20000000;
        const MAXIMUM_MODE         = 0x40000000;
    }
}

bitflags! {
    /// Official ocgcore `RESET_*` flags.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
    pub struct ResetFlag: u32 {
        const EVENT        = 0x00001000;
        const CARD         = 0x00002000;
        const CODE         = 0x00004000;
        const COPY         = 0x00008000;
        const DISABLE      = 0x00010000;
        const TURN_SET     = 0x00020000;
        const TOGRAVE      = 0x00040000;
        const REMOVE       = 0x00080000;
        const TEMP_REMOVE  = 0x00100000;
        const TOHAND       = 0x00200000;
        const TODECK       = 0x00400000;
        const LEAVE        = 0x00800000;
        const TOFIELD      = 0x01000000;
        const CONTROL      = 0x02000000;
        const OVERLAY      = 0x04000000;
        const MSCHANGE     = 0x08000000;
        const SELF_TURN    = 0x10000000;
        const OPPO_TURN    = 0x20000000;
        const PHASE        = 0x40000000;
        const CHAIN        = 0x80000000;
        const STANDARD     = 0x01FE0000;
        const STANDARD_EXC_GRAVE = 0x01FA0000;
    }
}

/// Official `EVENT_*` codes matching ocgcore.
pub mod event_code {
    pub const STARTUP: u32 = 1000;
    pub const FLIP: u32 = 1001;
    pub const FREE_CHAIN: u32 = 1002;
    pub const DESTROY: u32 = 1010;
    pub const REMOVE: u32 = 1011;
    pub const TO_HAND: u32 = 1012;
    pub const TO_DECK: u32 = 1013;
    pub const TO_GRAVE: u32 = 1014;
    pub const LEAVE_FIELD: u32 = 1015;
    pub const CHANGE_POS: u32 = 1016;
    pub const RELEASE: u32 = 1017;
    pub const DISCARD: u32 = 1018;
    pub const LEAVE_FIELD_P: u32 = 1019;
    pub const CHAIN_SOLVING: u32 = 1020;
    pub const CHAIN_ACTIVATING: u32 = 1021;
    pub const CHAIN_SOLVED: u32 = 1022;
    pub const CHAIN_NEGATED: u32 = 1024;
    pub const CHAIN_DISABLED: u32 = 1025;
    pub const CHAIN_END: u32 = 1026;
    pub const CHAINING: u32 = 1027;
    pub const BECOME_TARGET: u32 = 1028;
    pub const DESTROYED: u32 = 1029;
    pub const MOVE: u32 = 1030;
    pub const ADJUST: u32 = 1040;
    pub const SUMMON_SUCCESS: u32 = 1100;
    pub const FLIP_SUMMON_SUCCESS: u32 = 1101;
    pub const SPSUMMON_SUCCESS: u32 = 1102;
    pub const SUMMON: u32 = 1103;
    pub const FLIP_SUMMON: u32 = 1104;
    pub const SPSUMMON: u32 = 1105;
    pub const MSET: u32 = 1106;
    pub const SSET: u32 = 1107;
    pub const BE_MATERIAL: u32 = 1108;
    pub const BE_PRE_MATERIAL: u32 = 1109;
    pub const DRAW: u32 = 1110;
    pub const DAMAGE: u32 = 1111;
    pub const RECOVER: u32 = 1112;
    pub const PREDICT: u32 = 1113;
    pub const SUMMON_NEGATED: u32 = 1114;
    pub const FLIP_SUMMON_NEGATED: u32 = 1115;
    pub const SPSUMMON_NEGATED: u32 = 1116;
    pub const CONTROL_CHANGED: u32 = 1120;
    pub const EQUIP: u32 = 1121;
    pub const ATTACK_ANNOUNCE: u32 = 1130;
    pub const BE_BATTLE_TARGET: u32 = 1131;
    pub const BATTLE_START: u32 = 1132;
    pub const BATTLE_CONFIRM: u32 = 1133;
    pub const PRE_DAMAGE_CALCULATE: u32 = 1134;
    pub const DAMAGE_CALCULATING: u32 = 1135;
    pub const PRE_BATTLE_DAMAGE: u32 = 1136;
    pub const BATTLE_DAMAGE: u32 = 1138;
    pub const BATTLED: u32 = 1139;
    pub const BATTLE_DESTROYING: u32 = 1140;
    pub const BATTLE_DESTROYED: u32 = 1141;
    pub const DAMAGE_STEP_END: u32 = 1142;
    pub const ATTACK_DISABLED: u32 = 1143;
    pub const BATTLE_DESTROYED_CONFIRM: u32 = 1144;
    pub const PHASE_START: u32 = 4096; // 0x1000 + phase
    pub const PHASE: u32 = 8192;       // 0x2000 + phase
    pub const CUSTOM: u32 = 0x10000000;
}

/// Official `EFFECT_*` continuous/rule modifier codes matching ocgcore.
pub mod effect_code {
    pub const IMMUNE_EFFECT: u32 = 1;
    pub const DISABLE: u32 = 2;
    pub const CANNOT_DISABLE: u32 = 3;
    pub const SET_CONTROL: u32 = 4;
    pub const CANNOT_CHANGE_CONTROL: u32 = 5;
    pub const CANNOT_ACTIVATE: u32 = 6;
    pub const CANNOT_TRIGGER: u32 = 7;
    pub const DISABLE_EFFECT: u32 = 8;
    pub const DISABLE_CHAIN: u32 = 9;
    pub const DISABLE_TRAPMONSTER: u32 = 10;
    pub const CANNOT_INACTIVATE: u32 = 12;
    pub const CANNOT_DISEFFECT: u32 = 13;
    pub const CANNOT_CHANGE_POSITION: u32 = 14;
    pub const TRAP_ACT_IN_HAND: u32 = 15;
    pub const TRAP_ACT_IN_SET_TURN: u32 = 16;
    pub const REMAIN_FIELD: u32 = 17;
    pub const MONSTER_SSET: u32 = 18;
    pub const QP_ACT_IN_SET_TURN: u32 = 19;
    pub const CANNOT_SUMMON: u32 = 20;
    pub const CANNOT_FLIP_SUMMON: u32 = 21;
    pub const CANNOT_SPECIAL_SUMMON: u32 = 22;
    pub const CANNOT_MSET: u32 = 23;
    pub const CANNOT_SSET: u32 = 24;
    pub const CANNOT_DRAW: u32 = 25;
    pub const CANNOT_DISABLE_SUMMON: u32 = 26;
    pub const CANNOT_DISABLE_SPSUMMON: u32 = 27;
    pub const SET_SUMMON_COUNT_LIMIT: u32 = 28;
    pub const EXTRA_SUMMON_COUNT: u32 = 29;
    pub const SPSUMMON_CONDITION: u32 = 30;
    pub const REVIVE_LIMIT: u32 = 31;
    pub const SUMMON_PROC: u32 = 32;
    pub const LIMIT_SUMMON_PROC: u32 = 33;
    pub const SPSUMMON_PROC: u32 = 34;
    pub const EXTRA_SET_COUNT: u32 = 35;
    pub const SET_PROC: u32 = 36;
    pub const LIMIT_SET_PROC: u32 = 37;
    pub const LIGHT_OF_INTERVENTION: u32 = 38;
    pub const CANNOT_DISABLE_FLIP_SUMMON: u32 = 39;
    pub const INDESTRUCTABLE: u32 = 40;
    pub const INDESTRUCTABLE_EFFECT: u32 = 41;
    pub const INDESTRUCTABLE_BATTLE: u32 = 42;
    pub const UNRELEASABLE_SUM: u32 = 43;
    pub const UNRELEASABLE_NONSUM: u32 = 44;
    pub const DESTROY_SUBSTITUTE: u32 = 45;
    pub const CANNOT_RELEASE: u32 = 46;
    pub const INDESTRUCTABLE_COUNT: u32 = 47;
    pub const UNRELEASABLE_EFFECT: u32 = 48;
    pub const DESTROY_REPLACE: u32 = 50;
    pub const RELEASE_REPLACE: u32 = 51;
    pub const SEND_REPLACE: u32 = 52;
    pub const CANNOT_DISCARD_HAND: u32 = 55;
    pub const CANNOT_DISCARD_DECK: u32 = 56;
    pub const CANNOT_USE_AS_COST: u32 = 57;
    pub const CANNOT_PLACE_COUNTER: u32 = 58;
    pub const CANNOT_TO_GRAVE_AS_COST: u32 = 59;
    pub const LEAVE_FIELD_REDIRECT: u32 = 60;
    pub const TO_HAND_REDIRECT: u32 = 61;
    pub const TO_DECK_REDIRECT: u32 = 62;
    pub const TO_GRAVE_REDIRECT: u32 = 63;
    pub const REMOVE_REDIRECT: u32 = 64;
    pub const CANNOT_TO_HAND: u32 = 65;
    pub const CANNOT_TO_DECK: u32 = 66;
    pub const CANNOT_REMOVE: u32 = 67;
    pub const CANNOT_TO_GRAVE: u32 = 68;
    pub const CANNOT_TURN_SET: u32 = 69;
    pub const CANNOT_BE_BATTLE_TARGET: u32 = 70;
    pub const CANNOT_BE_EFFECT_TARGET: u32 = 71;
    pub const IGNORE_BATTLE_TARGET: u32 = 72;
    pub const CANNOT_DIRECT_ATTACK: u32 = 73;
    pub const DIRECT_ATTACK: u32 = 74;
    pub const DUAL_STATUS: u32 = 75;
    pub const EQUIP_LIMIT: u32 = 76;
    pub const DUAL_SUMMONABLE: u32 = 77;
    pub const REVERSE_DAMAGE: u32 = 80;
    pub const REVERSE_RECOVER: u32 = 81;
    pub const CHANGE_DAMAGE: u32 = 82;
    pub const REFLECT_DAMAGE: u32 = 83;
    pub const CANNOT_ATTACK: u32 = 85;
    pub const CANNOT_ATTACK_ANNOUNCE: u32 = 86;
    pub const CANNOT_CHANGE_POS_E: u32 = 87;
    pub const ACTIVATE_COST: u32 = 90;
    pub const SUMMON_COST: u32 = 91;
    pub const SPSUMMON_COST: u32 = 92;
    pub const FLIPSUMMON_COST: u32 = 93;
    pub const MSET_COST: u32 = 94;
    pub const SSET_COST: u32 = 95;
    pub const ATTACK_COST: u32 = 96;
    pub const UPDATE_ATTACK: u32 = 100;
    pub const SET_ATTACK: u32 = 101;
    pub const SET_ATTACK_FINAL: u32 = 102;
    pub const SET_BASE_ATTACK: u32 = 103;
    pub const UPDATE_DEFENSE: u32 = 104;
    pub const SET_DEFENSE: u32 = 105;
    pub const SET_DEFENSE_FINAL: u32 = 106;
    pub const SET_BASE_DEFENSE: u32 = 107;
    pub const REVERSE_UPDATE: u32 = 108;
    pub const SWAP_AD: u32 = 109;
    pub const SWAP_BASE_AD: u32 = 110;
    pub const SET_BASE_ATTACK_FINAL: u32 = 111;
    pub const SET_BASE_DEFENSE_FINAL: u32 = 112;
    pub const ADD_CODE: u32 = 113;
    pub const CHANGE_CODE: u32 = 114;
    pub const ADD_TYPE: u32 = 115;
    pub const REMOVE_TYPE: u32 = 116;
    pub const CHANGE_TYPE: u32 = 117;
    pub const ADD_RACE: u32 = 120;
    pub const REMOVE_RACE: u32 = 121;
    pub const CHANGE_RACE: u32 = 122;
    pub const ADD_ATTRIBUTE: u32 = 125;
    pub const REMOVE_ATTRIBUTE: u32 = 126;
    pub const CHANGE_ATTRIBUTE: u32 = 127;
    pub const UPDATE_LEVEL: u32 = 130;
    pub const CHANGE_LEVEL: u32 = 131;
    pub const UPDATE_RANK: u32 = 132;
    pub const CHANGE_RANK: u32 = 133;
    pub const UPDATE_LSCALE: u32 = 134;
    pub const CHANGE_LSCALE: u32 = 135;
    pub const UPDATE_RSCALE: u32 = 136;
    pub const CHANGE_RSCALE: u32 = 137;
    pub const SET_POSITION: u32 = 140;
    pub const SELF_DESTROY: u32 = 141;
    pub const SELF_TOGRAVE: u32 = 142;
    pub const DOUBLE_TRIBUTE: u32 = 150;
    pub const DECREASE_TRIBUTE: u32 = 151;
    pub const EXTRA_ATTACK: u32 = 192;
    pub const EXTRA_ATTACK_MONSTER: u32 = 193;
    pub const PIERCE: u32 = 195;
    pub const DEFENSE_ATTACK: u32 = 196;
    pub const MUST_ATTACK: u32 = 197;
    pub const FIRST_ATTACK: u32 = 198;
    pub const AVOID_BATTLE_DAMAGE: u32 = 200;
    pub const NO_BATTLE_DAMAGE: u32 = 201;
    pub const REFLECT_BATTLE_DAMAGE: u32 = 202;
    pub const BOTH_BATTLE_DAMAGE: u32 = 203;
    pub const ALSO_BATTLE_DAMAGE: u32 = 204;
    pub const CHANGE_BATTLE_DAMAGE: u32 = 205;
    pub const NO_EFFECT_DAMAGE: u32 = 206;
    pub const HAND_LIMIT: u32 = 210;
    pub const DRAW_COUNT: u32 = 211;
    pub const SKIP_DP: u32 = 212;
    pub const SKIP_SP: u32 = 213;
    pub const SKIP_M1: u32 = 214;
    pub const SKIP_BP: u32 = 215;
    pub const SKIP_M2: u32 = 216;
    pub const CANNOT_BP: u32 = 217;
    pub const CANNOT_M2: u32 = 218;
    pub const CANNOT_EP: u32 = 219;
    pub const SKIP_TURN: u32 = 220;
    pub const PUBLIC: u32 = 240;
    pub const COUNTER_PERMIT: u32 = 241;
    pub const COUNTER_LIMIT: u32 = 242;
    pub const RCOUNTER_CHANGE: u32 = 243;
    pub const FUSION_MATERIAL: u32 = 250;
    pub const CHAIN_MATERIAL: u32 = 251;
    pub const SYNCHRO_MATERIAL: u32 = 252;
    pub const XYZ_MATERIAL: u32 = 253;
    pub const FUSION_SUBSTITUTE: u32 = 254;
    pub const CANNOT_BE_FUSION_MATERIAL: u32 = 255;
    pub const CANNOT_BE_SYNCHRO_MATERIAL: u32 = 256;
    pub const CANNOT_BE_XYZ_MATERIAL: u32 = 257;
    pub const CANNOT_BE_LINK_MATERIAL: u32 = 258;
    pub const EXTRA_LINK_MATERIAL: u32 = 259;
    pub const CAN_BE_TUNER: u32 = 260;
    pub const UNION_LIMIT: u32 = 270;
    pub const MATCH_KILL: u32 = 280;
    pub const CAN_WIN_DUEL: u32 = 281;
}

bitflags! {
    /// Duel configuration flags supporting Master Rules 1-5, GOAT, Edison, Speed, Rush, Turbo, Action, Tag, Relay, Manual.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
    pub struct DuelFlags: u64 {
        const NONE                 = 0;
        /// Legacy MR1/MR2 Ignition Effect Priority on Summon (GOAT & Edison!)
        const OBSOLETE_IGNITION    = 1 << 0;
        /// Turn 1 player draws a card (MR1/MR2, GOAT, Edison)
        const TURN1_DRAW           = 1 << 1;
        /// Single shared Field Spell (MR1/MR2)
        const SHARED_FIELD_SPELL   = 1 << 2;
        /// Separate Pendulum Zones outside S/T zones (MR3)
        const SEPARATE_PZONE       = 1 << 3;
        /// Extra Monster Zones active (MR4/MR5)
        const EXTRA_MONSTER_ZONES  = 1 << 4;
        /// All Extra Deck monsters require EMZ or Linked zone (MR4 strict)
        const MR4_EXTRA_LOCK       = 1 << 5;
        /// 3 Monster / 3 S&T columns (Speed Duel & Rush Duel)
        const THREE_COLUMNS_FIELD  = 1 << 6;
        /// Skip Main Phase 2 (Speed Duel & Rush Duel)
        const NO_MAIN_PHASE_2      = 1 << 7;
        /// Rush Duel rules (draw-to-5, unlimited Normal Summons, Maximum Summon, no Chains)
        const RUSH_DUEL            = 1 << 8;
        /// Speed Duel / Duel Links rules (Skill Cards, 4000 LP, 4 hand)
        const SPEED_DUEL           = 1 << 9;
        /// Turbo / Riding Duel (Speed World 2 + Speed Counters)
        const TURBO_DUEL           = 1 << 10;
        /// Action Duel (Action Field + Action Cards)
        const ACTION_DUEL          = 1 << 11;
        /// Tag Duel (2v2, 4 players, shared field & 16000 LP)
        const TAG_MODE             = 1 << 12;
        /// Relay Duel (team gauntlet preserving field)
        const RELAY_MODE           = 1 << 13;
        /// Manual / Judge sandbox mode enabled
        const MANUAL_MODE          = 1 << 14;
        /// Disable deck shuffling (for Puzzle / Hand Test)
        const NO_SHUFFLE_DECK      = 1 << 15;
        /// Genesys point-system format
        const GENESYS_MODE         = 1 << 16;
        /// No Pendulum or Link summoning (optional Genesys / retro toggle)
        const NO_PENDULUM_OR_LINK  = 1 << 17;
    }
}

/// YGO Omega's 4-mode Chain Control settings!
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum ChainControlMode {
    /// Prompt on standard meaningful chain windows (default Omega behavior)
    #[default]
    Auto,
    /// Prompt at every legal fast-effect timing window (hold 'A' in Omega)
    All,
    /// Prompt only when a card has a legal, resolvable activation without missing timing
    Legal,
    /// Automatically pass all optional chain prompts (hold 'S' in Omega)
    Off,
}

/// Reason a duel ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WinReason {
    LpZero,
    DeckOut,
    Exodia,
    FinalCountdown,
    DestinyBoard,
    SpecialWinEffect,
    Surrender,
    TimeLimit,
    ManualJudgeDecision,
    DrawGame,
}
