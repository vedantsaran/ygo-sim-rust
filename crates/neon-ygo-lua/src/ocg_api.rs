use std::collections::BTreeMap;
use neon_ygo_core::*;

use crate::value::{LuaVal, TableId, TableKey};
use crate::vm::{build_callback_args, LuaError, LuaVm};

/// Complete `ScriptHost` backed by our 100% pure-Rust Lua 5.4 VM!
/// Loads card scripts by passcode (from an in-memory map or filesystem directory) and executes
/// `s.initial_effect(c)`, `condition`, `cost`, `target`, `operation`, and `value` callbacks!
pub struct LuaScriptHost {
    pub vm: LuaVm,
    pub scripts: BTreeMap<CardCode, String>,
    pub script_dirs: Vec<std::path::PathBuf>,
    pub core_script_dir: Option<std::path::PathBuf>,
    pub core_libs_loaded: bool,
    pub loaded_tables: BTreeMap<CardCode, TableId>,
    pub card_catalog: BTreeMap<CardCode, CardData>,
}

impl Default for LuaScriptHost {
    fn default() -> Self {
        Self::new()
    }
}

impl LuaScriptHost {
    pub fn new() -> Self {
        Self {
            vm: LuaVm::new(),
            scripts: BTreeMap::new(),
            script_dirs: Vec::new(),
            core_script_dir: None,
            core_libs_loaded: false,
            loaded_tables: BTreeMap::new(),
            card_catalog: BTreeMap::new(),
        }
    }

    pub fn register_script(&mut self, code: CardCode, lua_source: impl Into<String>) {
        self.scripts.insert(code, lua_source.into());
    }

    pub fn add_script_dir(&mut self, dir: impl Into<std::path::PathBuf>) {
        let p = dir.into();
        if !self.vm.script_search_dirs.contains(&p) {
            self.vm.script_search_dirs.push(p.clone());
        }
        if !self.script_dirs.contains(&p) {
            self.script_dirs.push(p);
        }
    }

    pub fn set_core_script_dir(&mut self, dir: impl Into<std::path::PathBuf>) {
        let p = dir.into();
        self.add_script_dir(p.clone());
        self.core_script_dir = Some(p);
    }

    /// Load ProjectIgnis/EDOPro/YGO Omega core Lua libraries (`constant.lua`, `archetype_setcode_constants.lua`,
    /// `card_counter_constants.lua`, `utility.lua`, `proc_*.lua`, `cards_specific_functions.lua`)!
    pub fn load_core_libraries(&mut self) -> Result<usize, LuaError> {
        if self.core_libs_loaded {
            return Ok(0);
        }
        let Some(ref root) = self.core_script_dir.clone() else {
            return Ok(0);
        };
        let ordered = [
            "constant.lua",
            "archetype_setcode_constants.lua",
            "card_counter_constants.lua",
            "utility.lua",
            "proc_normal.lua",
            "proc_ritual.lua",
            "proc_fusion.lua",
            "proc_fusion_spell.lua",
            "proc_synchro.lua",
            "proc_xyz.lua",
            "proc_pendulum.lua",
            "proc_link.lua",
            "proc_equip.lua",
            "proc_gemini.lua",
            "proc_spirit.lua",
            "proc_union.lua",
            "proc_maximum.lua",
            "proc_rush.lua",
            "proc_skill.lua",
            "proc_persistent.lua",
            "proc_workaround.lua",
            "cards_specific_functions.lua",
        ];
        let mut loaded = 0usize;
        for fname in ordered {
            let path = root.join(fname);
            if path.is_file() {
                if let Ok(src) = std::fs::read_to_string(&path) {
                    self.vm.loaded_helper_scripts.insert(fname.to_string());
                    match self.vm.exec(&src, None) {
                        Ok(_) => loaded += 1,
                        Err(e) => {
                            self.vm.output_log.push(format!("core lib {fname}: {e}"));
                        }
                    }
                }
            }
        }
        self.core_libs_loaded = true;
        Ok(loaded)
    }

    pub fn register_card_data(&mut self, data: CardData) {
        self.vm.card_catalog.insert(data.code, data.clone());
        self.card_catalog.insert(data.code, data);
    }

    pub fn find_script(&self, code: CardCode) -> Option<String> {
        if let Some(src) = self.scripts.get(&code) {
            return Some(src.clone());
        }
        for dir in &self.script_dirs {
            let p = dir.join(format!("c{code}.lua"));
            if let Ok(src) = std::fs::read_to_string(p) {
                return Some(src);
            }
        }
        None
    }

    /// Execute an official YGOPro / YGO Omega `.lua` Puzzle script (`Debug.ReloadFieldBegin`, `Debug.AddCard`, etc.)!
    pub fn run_puzzle_script(&mut self, engine: &mut DuelEngine, src: &str) -> Result<(), LuaError> {
        let _ = self.vm.exec(src, Some(engine))?;
        Ok(())
    }
}

impl ScriptHost for LuaScriptHost {
    fn init_card(&mut self, engine: &mut DuelEngine, card_id: CardId) {
        let Some(code) = engine.card(card_id).map(|c| c.data.code) else {
            return;
        };
        let tid = if let Some(&existing) = self.loaded_tables.get(&code) {
            existing
        } else if let Some(src) = self.find_script(code) {
            match self.vm.load_card_script(code, &src, engine) {
                Ok(t) => {
                    self.loaded_tables.insert(code, t);
                    t
                }
                Err(e) => {
                    self.vm.output_log.push(format!("script c{code}.lua error: {e}"));
                    return;
                }
            }
        } else {
            return;
        };

        self.vm.card_metatables.insert(card_id, tid);
        let init_fn = self.vm.get_table_val(tid, &TableKey::Str("initial_effect".into()));
        if !matches!(init_fn, LuaVal::Nil) {
            let mut eng_opt = Some(engine);
            if let Err(e) = self.vm.call_value(init_fn, vec![LuaVal::Card(card_id)], &mut eng_opt) {
                self.vm.output_log.push(format!("c{code}.initial_effect error: {e}"));
            }
        }
    }

    fn eval_condition(
        &mut self,
        engine: &mut DuelEngine,
        effect_id: EffectId,
        tp: PlayerId,
        event: Option<&DuelEvent>,
    ) -> bool {
        let Some(EffectCallback::LuaFunc(fid)) = engine.state.effects.get(&effect_id).map(|e| e.condition.clone()) else {
            return true;
        };
        let args = build_callback_args(&mut self.vm, effect_id, tp, event, None);
        let mut eng_opt = Some(engine);
        match self.vm.call_closure(fid, args, &mut eng_opt) {
            Ok(vals) => vals.first().map(|v| v.is_truthy()).unwrap_or(false),
            Err(e) => {
                self.vm.output_log.push(format!("condition error: {e}"));
                false
            }
        }
    }

    fn eval_cost(
        &mut self,
        engine: &mut DuelEngine,
        effect_id: EffectId,
        tp: PlayerId,
        event: Option<&DuelEvent>,
        chk: i32,
    ) -> bool {
        let Some(EffectCallback::LuaFunc(fid)) = engine.state.effects.get(&effect_id).map(|e| e.cost.clone()) else {
            return true;
        };
        let args = build_callback_args(&mut self.vm, effect_id, tp, event, Some(chk));
        let mut eng_opt = Some(engine);
        match self.vm.call_closure(fid, args, &mut eng_opt) {
            Ok(vals) => {
                if chk == 0 {
                    vals.first().map(|v| v.is_truthy()).unwrap_or(true)
                } else {
                    true
                }
            }
            Err(e) => {
                self.vm.output_log.push(format!("cost error: {e}"));
                false
            }
        }
    }

    fn eval_target(
        &mut self,
        engine: &mut DuelEngine,
        effect_id: EffectId,
        tp: PlayerId,
        event: Option<&DuelEvent>,
        chk: i32,
    ) -> bool {
        let Some(EffectCallback::LuaFunc(fid)) = engine.state.effects.get(&effect_id).map(|e| e.target.clone()) else {
            return true;
        };
        let args = build_callback_args(&mut self.vm, effect_id, tp, event, Some(chk));
        let mut eng_opt = Some(engine);
        match self.vm.call_closure(fid, args, &mut eng_opt) {
            Ok(vals) => {
                if chk == 0 {
                    vals.first().map(|v| v.is_truthy()).unwrap_or(true)
                } else {
                    true
                }
            }
            Err(e) => {
                self.vm.output_log.push(format!("target error: {e}"));
                false
            }
        }
    }

    fn eval_operation(
        &mut self,
        engine: &mut DuelEngine,
        effect_id: EffectId,
        tp: PlayerId,
        event: Option<&DuelEvent>,
    ) {
        let Some(EffectCallback::LuaFunc(fid)) = engine.state.effects.get(&effect_id).map(|e| e.operation.clone()) else {
            return;
        };
        let args = build_callback_args(&mut self.vm, effect_id, tp, event, None);
        let mut eng_opt = Some(engine);
        if let Err(e) = self.vm.call_closure(fid, args, &mut eng_opt) {
            self.vm.output_log.push(format!("operation error: {e}"));
        }
    }

    fn eval_value(
        &mut self,
        engine: &mut DuelEngine,
        effect_id: EffectId,
        target_card: Option<CardId>,
    ) -> i64 {
        let Some(EffectValue::Callback(EffectCallback::LuaFunc(fid))) =
            engine.state.effects.get(&effect_id).map(|e| e.value.clone())
        else {
            return 0;
        };
        let args = vec![
            LuaVal::Effect(effect_id),
            target_card.map(LuaVal::Card).unwrap_or(LuaVal::Nil),
        ];
        let mut eng_opt = Some(engine);
        match self.vm.call_closure(fid, args, &mut eng_opt) {
            Ok(vals) => vals.first().map(|v| v.as_int()).unwrap_or(0),
            Err(e) => {
                self.vm.output_log.push(format!("value error: {e}"));
                0
            }
        }
    }
}

/// Register ALL official `ocgcore` / EDOPro / YGO Omega constants and API namespaces in `LuaVm`.
pub fn register_ocg_globals(vm: &mut LuaVm) {
    // 1. Namespaces: Duel, Card, Effect, Group, Debug, Auxiliary / aux, Fusion, Synchro, Xyz, Link, Ritual, Pendulum
    let namespaces: &[(&str, &[&str])] = &[
        ("Duel", &[
            "Draw", "DiscardHand", "SendtoGrave", "SendtoHand", "SendtoDeck", "Remove",
            "Destroy", "SpecialSummon", "SpecialSummonStep", "SpecialSummonComplete",
            "Damage", "Recover", "SetLP", "GetLP", "PayLPCost", "CheckLPCost",
            "SelectMatchingCard", "SelectTarget", "GetMatchingGroup", "GetMatchingGroupCount",
            "IsExistingMatchingCard", "IsExistingTarget", "GetFieldGroup", "GetFieldGroupCount",
            "GetLocationCount", "GetLocationCountFromEx", "SetOperationInfo", "GetFirstTarget",
            "GetChainInfo", "NegateActivation", "NegateEffect", "ChangePosition", "Equip",
            "ConfirmCards", "ShuffleDeck", "ShuffleHand", "GetTurnPlayer", "GetTurnCount",
            "GetCurrentPhase", "GetCurrentChain", "TossCoin", "TossDice", "Hint",
            "RegisterEffect", "IsPlayerCanDraw", "IsPlayerCanSpecialSummon",
            "IsPlayerCanSpecialSummonMonster", "Release", "CheckReleaseGroup", "SelectReleaseGroup",
            "GetAttacker", "GetAttackTarget", "NegateAttack", "Win", "CreateToken", "Overlay",
            "MoveToField", "SetChainLimit", "GetOperatedGroup", "CheckTiming",
            "GetTargetCards", "SetTargetCard", "SetTargetPlayer", "SetTargetParam",
            "GetFirstMatchingCard", "RemoveOverlayCard", "CheckRemoveOverlayCard",
            "LoadScript", "LoadCardScript", "LoadCardScriptAlias", "EnableUnofficialProc",
            "EnableGlobalFlag", "AddCustomActivityCounter", "GetCustomActivityCount",
            "ImportScript", "RegisterFlagEffect", "GetFlagEffect", "ResetFlagEffect",
            "SetFlagEffectLabel", "GetFlagEffectLabel", "GetTargetCount",
            "GetReasonEffect", "IsDuelType", "GetMasterRule", "GetEnvironment",
            "GetOperationInfo", "GetChainMaterial", "CheckEvent", "GetTurnID",
        ]),
        ("Card", &[
            "RegisterEffect", "GetCode", "GetOriginalCode", "GetType", "GetAttribute", "GetRace",
            "GetLevel", "GetRank", "GetLink", "GetAttack", "GetDefense", "GetBaseAttack",
            "GetBaseDefense", "GetLocation", "GetSequence", "GetControler", "GetOwner",
            "GetPosition", "IsFaceup", "IsFacedown", "IsAttackPos", "IsDefensePos", "IsType",
            "IsAttribute", "IsRace", "IsSetCard", "IsCode", "IsMonster", "IsSpell", "IsTrap",
            "IsAbleToHand", "IsAbleToGrave", "IsAbleToRemove", "IsAbleToDeck",
            "IsCanBeSpecialSummoned", "IsCanBeEffectTarget", "IsDestructable", "IsRelateToEffect",
            "IsRelateToBattle", "IsPreviousLocation", "IsReason", "IsStatus", "IsSummonType",
            "GetOverlayGroup", "GetOverlayCount", "CheckRemoveOverlayCard", "RemoveOverlayCard",
            "GetCounter", "AddCounter", "RemoveCounter", "GetEquipTarget", "GetEquipGroup",
            "GetLinkedGroup", "GetColumnGroup", "GetLeftScale", "GetRightScale",
            "RegisterFlagEffect", "GetFlagEffect", "ResetFlagEffect", "IsDisabled", "IsPublic",
            "IsOnField", "IsLocation", "IsControler", "IsPosition", "IsReleasable", "IsDiscardable",
            "EnableReviveLimit", "EnablePendulumAttribute", "EnableCounterPermit", "SetCounterLimit",
            "SetUniqueOnField", "SetSPSummonOnce", "AddMonsterAttribute", "SetStatus",
            "EnableUnsummonable", "SetFlagEffectLabel", "GetFlagEffectLabel", "GetOriginalCodeRule",
            "ListsCode", "ListsArchetype", "IsOriginalCodeRule", "IsSummonCode", "GetMetatable",
        ]),
        ("Effect", &[
            "CreateEffect", "GlobalEffect", "Clone", "SetType", "SetCode", "SetCategory", "SetProperty",
            "SetRange", "SetTargetRange", "SetAbsoluteRange", "SetCountLimit", "SetCondition", "SetCost",
            "SetTarget", "SetOperation", "SetValue", "SetReset", "SetDescription",
            "SetHintTiming", "SetLabel", "GetLabel", "SetLabelObject", "GetLabelObject",
            "GetHandler", "GetHandlerPlayer", "GetOwner", "GetOwnerPlayer", "GetCode",
            "GetType", "GetCategory", "GetProperty", "Reset",
        ]),
        ("Group", &[
            "CreateGroup", "FromCards", "Clone", "AddCard", "RemoveCard", "Merge", "Sub",
            "GetCount", "GetFirst", "GetNext", "Filter", "FilterCount", "IsExists",
            "Select", "FilterSelect", "GetSum", "GetMaxGroup", "GetMinGroup",
            "IsContains", "ForEach", "Clear", "KeepAlive", "DeleteGroup",
        ]),
        ("Debug", &[
            "SetAIName", "ReloadFieldBegin", "AddCard", "ReloadFieldEnd", "ShowHint",
            "PreSummon", "PreEquip", "PreAddCounter", "Message",
        ]),
        ("Auxiliary", &[
            "Stringid", "FilterBoolFunction", "TargetBoolFunction", "NecroValleyFilter",
            "EnablePendulumAttribute", "AddFusionProcMix", "AddSynchroProcedure",
            "AddXyzProcedure", "AddLinkProcedure", "AddRitualProcGreater",
        ]),
    ];

    for &(ns, funcs) in namespaces {
        let tid = vm.alloc_table();
        for &f in funcs {
            vm.set_table_field(tid, f, LuaVal::NativeFn(format!("{ns}.{f}")));
        }
        vm.set_global(ns, LuaVal::Table(tid));
        if ns == "Auxiliary" {
            vm.set_global("aux", LuaVal::Table(tid));
        }
    }

    // EDOPro modern procedure namespaces (`Fusion`, `Synchro`, `Xyz`, `Link`, `Ritual`, `Pendulum`)
    for (ns, f) in [
        ("Fusion", "AddProcMix"),
        ("Synchro", "AddProcedure"),
        ("Xyz", "AddProcedure"),
        ("Link", "AddProcedure"),
        ("Ritual", "AddProcGreater"),
        ("Pendulum", "AddProcedure"),
    ] {
        let tid = vm.alloc_table();
        vm.set_table_field(tid, f, LuaVal::NativeFn(format!("Auxiliary.{ns}_{f}")));
        vm.set_global(ns, LuaVal::Table(tid));
    }

    // 2. Register ALL official ocgcore constants
    let constants: &[(&str, i64)] = &[
        // Locations
        ("LOCATION_DECK", Location::DECK.bits() as i64),
        ("LOCATION_HAND", Location::HAND.bits() as i64),
        ("LOCATION_MZONE", Location::MZONE.bits() as i64),
        ("LOCATION_SZONE", Location::SZONE.bits() as i64),
        ("LOCATION_GRAVE", Location::GRAVE.bits() as i64),
        ("LOCATION_REMOVED", Location::REMOVED.bits() as i64),
        ("LOCATION_EXTRA", Location::EXTRA.bits() as i64),
        ("LOCATION_OVERLAY", Location::OVERLAY.bits() as i64),
        ("LOCATION_ONFIELD", Location::ONFIELD.bits() as i64),
        ("LOCATION_FZONE", Location::FZONE.bits() as i64),
        ("LOCATION_PZONE", Location::PZONE.bits() as i64),
        ("LOCATION_SKILL", Location::SKILL.bits() as i64),
        ("LOCATION_ALL", Location::ALL.bits() as i64),
        // Positions
        ("POS_FACEUP_ATTACK", Position::FACEUP_ATTACK.bits() as i64),
        ("POS_FACEDOWN_ATTACK", Position::FACEDOWN_ATTACK.bits() as i64),
        ("POS_FACEUP_DEFENSE", Position::FACEUP_DEFENSE.bits() as i64),
        ("POS_FACEDOWN_DEFENSE", Position::FACEDOWN_DEFENSE.bits() as i64),
        ("POS_FACEUP", Position::FACEUP.bits() as i64),
        ("POS_FACEDOWN", Position::FACEDOWN.bits() as i64),
        ("POS_ATTACK", Position::ATTACK.bits() as i64),
        ("POS_DEFENSE", Position::DEFENSE.bits() as i64),
        // Card Types
        ("TYPE_MONSTER", CardType::MONSTER.bits() as i64),
        ("TYPE_SPELL", CardType::SPELL.bits() as i64),
        ("TYPE_TRAP", CardType::TRAP.bits() as i64),
        ("TYPE_NORMAL", CardType::NORMAL.bits() as i64),
        ("TYPE_EFFECT", CardType::EFFECT.bits() as i64),
        ("TYPE_FUSION", CardType::FUSION.bits() as i64),
        ("TYPE_RITUAL", CardType::RITUAL.bits() as i64),
        ("TYPE_SPIRIT", CardType::SPIRIT.bits() as i64),
        ("TYPE_UNION", CardType::UNION.bits() as i64),
        ("TYPE_DUAL", CardType::GEMINI.bits() as i64),
        ("TYPE_GEMINI", CardType::GEMINI.bits() as i64),
        ("TYPE_TUNER", CardType::TUNER.bits() as i64),
        ("TYPE_SYNCHRO", CardType::SYNCHRO.bits() as i64),
        ("TYPE_TOKEN", CardType::TOKEN.bits() as i64),
        ("TYPE_MAXIMUM", CardType::MAXIMUM.bits() as i64),
        ("TYPE_QUICKPLAY", CardType::QUICKPLAY.bits() as i64),
        ("TYPE_CONTINUOUS", CardType::CONTINUOUS.bits() as i64),
        ("TYPE_EQUIP", CardType::EQUIP.bits() as i64),
        ("TYPE_FIELD", CardType::FIELD.bits() as i64),
        ("TYPE_COUNTER", CardType::COUNTER.bits() as i64),
        ("TYPE_FLIP", CardType::FLIP.bits() as i64),
        ("TYPE_TOON", CardType::TOON.bits() as i64),
        ("TYPE_XYZ", CardType::XYZ.bits() as i64),
        ("TYPE_PENDULUM", CardType::PENDULUM.bits() as i64),
        ("TYPE_SPSUMMON", CardType::SPSUMMON.bits() as i64),
        ("TYPE_LINK", CardType::LINK.bits() as i64),
        // Attributes
        ("ATTRIBUTE_EARTH", Attribute::EARTH.bits() as i64),
        ("ATTRIBUTE_WATER", Attribute::WATER.bits() as i64),
        ("ATTRIBUTE_FIRE", Attribute::FIRE.bits() as i64),
        ("ATTRIBUTE_WIND", Attribute::WIND.bits() as i64),
        ("ATTRIBUTE_LIGHT", Attribute::LIGHT.bits() as i64),
        ("ATTRIBUTE_DARK", Attribute::DARK.bits() as i64),
        ("ATTRIBUTE_DIVINE", Attribute::DIVINE.bits() as i64),
        // Races
        ("RACE_WARRIOR", Race::WARRIOR.bits() as i64),
        ("RACE_SPELLCASTER", Race::SPELLCASTER.bits() as i64),
        ("RACE_FAIRY", Race::FAIRY.bits() as i64),
        ("RACE_FIEND", Race::FIEND.bits() as i64),
        ("RACE_ZOMBIE", Race::ZOMBIE.bits() as i64),
        ("RACE_MACHINE", Race::MACHINE.bits() as i64),
        ("RACE_AQUA", Race::AQUA.bits() as i64),
        ("RACE_PYRO", Race::PYRO.bits() as i64),
        ("RACE_ROCK", Race::ROCK.bits() as i64),
        ("RACE_WINGEDBEAST", Race::WINGEDBEAST.bits() as i64),
        ("RACE_PLANT", Race::PLANT.bits() as i64),
        ("RACE_INSECT", Race::INSECT.bits() as i64),
        ("RACE_THUNDER", Race::THUNDER.bits() as i64),
        ("RACE_DRAGON", Race::DRAGON.bits() as i64),
        ("RACE_BEAST", Race::BEAST.bits() as i64),
        ("RACE_BEASTWARRIOR", Race::BEASTWARRIOR.bits() as i64),
        ("RACE_DINOSAUR", Race::DINOSAUR.bits() as i64),
        ("RACE_FISH", Race::FISH.bits() as i64),
        ("RACE_SEASERPENT", Race::SEASERPENT.bits() as i64),
        ("RACE_REPTILE", Race::REPTILE.bits() as i64),
        ("RACE_PSYCHIC", Race::PSYCHIC.bits() as i64),
        ("RACE_DIVINE", Race::DIVINE.bits() as i64),
        ("RACE_WYRM", Race::WYRM.bits() as i64),
        ("RACE_CYBERSE", Race::CYBERSE.bits() as i64),
        ("RACE_ILLUSION", Race::ILLUSION.bits() as i64),
        // Effect Types
        ("EFFECT_TYPE_SINGLE", EffectType::SINGLE.bits() as i64),
        ("EFFECT_TYPE_FIELD", EffectType::FIELD.bits() as i64),
        ("EFFECT_TYPE_EQUIP", EffectType::EQUIP.bits() as i64),
        ("EFFECT_TYPE_ACTIVATE", EffectType::ACTIVATE.bits() as i64),
        ("EFFECT_TYPE_FLIP", EffectType::FLIP.bits() as i64),
        ("EFFECT_TYPE_IGNITION", EffectType::IGNITION.bits() as i64),
        ("EFFECT_TYPE_TRIGGER_O", EffectType::TRIGGER_O.bits() as i64),
        ("EFFECT_TYPE_QUICK_O", EffectType::QUICK_O.bits() as i64),
        ("EFFECT_TYPE_TRIGGER_F", EffectType::TRIGGER_F.bits() as i64),
        ("EFFECT_TYPE_QUICK_F", EffectType::QUICK_F.bits() as i64),
        ("EFFECT_TYPE_CONTINUOUS", EffectType::CONTINUOUS.bits() as i64),
        ("EFFECT_TYPE_GRANT", EffectType::GRANT.bits() as i64),
        ("EFFECT_TYPE_TARGET", EffectType::TARGET.bits() as i64),
        ("EFFECT_TYPE_UNCLASSIFIED", EffectType::UNCLASSIFIED.bits() as i64),
        // Effect Flags
        ("EFFECT_FLAG_CARD_TARGET", EffectFlag::CARD_TARGET.bits() as i64),
        ("EFFECT_FLAG_PLAYER_TARGET", EffectFlag::PLAYER_TARGET.bits() as i64),
        ("EFFECT_FLAG_DELAY", EffectFlag::DELAY.bits() as i64),
        ("EFFECT_FLAG_DAMAGE_STEP", EffectFlag::DAMAGE_STEP.bits() as i64),
        ("EFFECT_FLAG_DAMAGE_CAL", EffectFlag::DAMAGE_CAL.bits() as i64),
        ("EFFECT_FLAG_SINGLE_RANGE", EffectFlag::SINGLE_RANGE.bits() as i64),
        ("EFFECT_FLAG_IGNORE_IMMUNE", EffectFlag::IGNORE_IMMUNE.bits() as i64),
        ("EFFECT_FLAG_CANNOT_DISABLE", EffectFlag::CANNOT_DISABLE.bits() as i64),
        ("EFFECT_FLAG_CANNOT_NEGATE", EffectFlag::CANNOT_NEGATE.bits() as i64),
        ("EFFECT_FLAG_UNCOPYABLE", EffectFlag::UNCOPYABLE.bits() as i64),
        ("EFFECT_FLAG_OATH", EffectFlag::OATH.bits() as i64),
        ("EFFECT_FLAG_SET_AVAILABLE", EffectFlag::SET_AVAILABLE.bits() as i64),
        ("EFFECT_FLAG_BOTH_SIDE", EffectFlag::BOTH_SIDE.bits() as i64),
        ("EFFECT_FLAG_CLIENT_HINT", EffectFlag::CLIENT_HINT.bits() as i64),
        // Effect Codes
        ("EFFECT_IMMUNE_EFFECT", effect_code::IMMUNE_EFFECT as i64),
        ("EFFECT_DISABLE", effect_code::DISABLE as i64),
        ("EFFECT_DISABLE_EFFECT", effect_code::DISABLE_EFFECT as i64),
        ("EFFECT_CANNOT_ACTIVATE", effect_code::CANNOT_ACTIVATE as i64),
        ("EFFECT_CANNOT_SPECIAL_SUMMON", effect_code::CANNOT_SPECIAL_SUMMON as i64),
        ("EFFECT_CANNOT_DRAW", effect_code::CANNOT_DRAW as i64),
        ("EFFECT_CANNOT_TO_HAND", effect_code::CANNOT_TO_HAND as i64),
        ("EFFECT_CANNOT_REMOVE", effect_code::CANNOT_REMOVE as i64),
        ("EFFECT_TO_GRAVE_REDIRECT", effect_code::TO_GRAVE_REDIRECT as i64),
        ("EFFECT_INDESTRUCTABLE_BATTLE", effect_code::INDESTRUCTABLE_BATTLE as i64),
        ("EFFECT_INDESTRUCTABLE_EFFECT", effect_code::INDESTRUCTABLE_EFFECT as i64),
        ("EFFECT_INDESTRUCTABLE", effect_code::INDESTRUCTABLE as i64),
        ("EFFECT_CANNOT_BE_EFFECT_TARGET", effect_code::CANNOT_BE_EFFECT_TARGET as i64),
        ("EFFECT_CANNOT_BE_BATTLE_TARGET", effect_code::CANNOT_BE_BATTLE_TARGET as i64),
        ("EFFECT_DIRECT_ATTACK", effect_code::DIRECT_ATTACK as i64),
        ("EFFECT_PIERCE", effect_code::PIERCE as i64),
        ("EFFECT_EXTRA_ATTACK", effect_code::EXTRA_ATTACK as i64),
        ("EFFECT_UPDATE_ATTACK", effect_code::UPDATE_ATTACK as i64),
        ("EFFECT_SET_ATTACK", effect_code::SET_ATTACK as i64),
        ("EFFECT_SET_ATTACK_FINAL", effect_code::SET_ATTACK_FINAL as i64),
        ("EFFECT_UPDATE_DEFENSE", effect_code::UPDATE_DEFENSE as i64),
        ("EFFECT_SET_DEFENSE", effect_code::SET_DEFENSE as i64),
        ("EFFECT_SWAP_AD", effect_code::SWAP_AD as i64),
        ("EFFECT_UPDATE_LEVEL", effect_code::UPDATE_LEVEL as i64),
        ("EFFECT_CHANGE_LEVEL", effect_code::CHANGE_LEVEL as i64),
        ("EFFECT_CHANGE_RACE", effect_code::CHANGE_RACE as i64),
        ("EFFECT_CHANGE_ATTRIBUTE", effect_code::CHANGE_ATTRIBUTE as i64),
        ("EFFECT_TRAP_ACT_IN_HAND", effect_code::TRAP_ACT_IN_HAND as i64),
        ("EFFECT_TRAP_ACT_IN_SET_TURN", effect_code::TRAP_ACT_IN_SET_TURN as i64),
        ("EFFECT_EXTRA_SUMMON_COUNT", effect_code::EXTRA_SUMMON_COUNT as i64),
        ("EFFECT_CANNOT_ATTACK", effect_code::CANNOT_ATTACK as i64),
        // Categories
        ("CATEGORY_DESTROY", Category::DESTROY.bits() as i64),
        ("CATEGORY_RELEASE", Category::RELEASE.bits() as i64),
        ("CATEGORY_REMOVE", Category::REMOVE.bits() as i64),
        ("CATEGORY_TOHAND", Category::TOHAND.bits() as i64),
        ("CATEGORY_TODECK", Category::TODECK.bits() as i64),
        ("CATEGORY_TOGRAVE", Category::TOGRAVE.bits() as i64),
        ("CATEGORY_SPECIAL_SUMMON", Category::SPECIAL_SUMMON.bits() as i64),
        ("CATEGORY_TOKEN", Category::TOKEN.bits() as i64),
        ("CATEGORY_DISABLE", Category::DISABLE.bits() as i64),
        ("CATEGORY_NEGATE", Category::NEGATED.bits() as i64),
        ("CATEGORY_DRAW", Category::DRAW.bits() as i64),
        ("CATEGORY_SEARCH", Category::SEARCH.bits() as i64),
        ("CATEGORY_EQUIP", Category::EQUIP.bits() as i64),
        ("CATEGORY_DAMAGE", Category::DAMAGE.bits() as i64),
        ("CATEGORY_RECOVER", Category::RECOVER.bits() as i64),
        ("CATEGORY_ATKCHANGE", Category::ATKCHANGE.bits() as i64),
        ("CATEGORY_DEFCHANGE", Category::DEFCHANGE.bits() as i64),
        ("CATEGORY_COUNTER", Category::COUNTER.bits() as i64),
        ("CATEGORY_FUSION_SUMMON", Category::FUSION_SUMMON.bits() as i64),
        // Reasons
        ("REASON_DESTROY", Reason::DESTROY.bits() as i64),
        ("REASON_RELEASE", Reason::RELEASE.bits() as i64),
        ("REASON_MATERIAL", Reason::MATERIAL.bits() as i64),
        ("REASON_SUMMON", Reason::SUMMON.bits() as i64),
        ("REASON_BATTLE", Reason::BATTLE.bits() as i64),
        ("REASON_EFFECT", Reason::EFFECT.bits() as i64),
        ("REASON_COST", Reason::COST.bits() as i64),
        ("REASON_RULE", Reason::RULE.bits() as i64),
        ("REASON_SPSUMMON", Reason::SPSUMMON.bits() as i64),
        ("REASON_DISCARD", Reason::DISCARD.bits() as i64),
        ("REASON_FUSION", Reason::FUSION.bits() as i64),
        ("REASON_SYNCHRO", Reason::SYNCHRO.bits() as i64),
        ("REASON_RITUAL", Reason::RITUAL.bits() as i64),
        ("REASON_XYZ", Reason::XYZ.bits() as i64),
        ("REASON_LINK", Reason::LINK.bits() as i64),
        // Events
        ("EVENT_FREE_CHAIN", event_code::FREE_CHAIN as i64),
        ("EVENT_FLIP", event_code::FLIP as i64),
        ("EVENT_DESTROYED", event_code::DESTROYED as i64),
        ("EVENT_TO_GRAVE", event_code::TO_GRAVE as i64),
        ("EVENT_TO_HAND", event_code::TO_HAND as i64),
        ("EVENT_REMOVE", event_code::REMOVE as i64),
        ("EVENT_LEAVE_FIELD", event_code::LEAVE_FIELD as i64),
        ("EVENT_SUMMON_SUCCESS", event_code::SUMMON_SUCCESS as i64),
        ("EVENT_SPSUMMON_SUCCESS", event_code::SPSUMMON_SUCCESS as i64),
        ("EVENT_FLIP_SUMMON_SUCCESS", event_code::FLIP_SUMMON_SUCCESS as i64),
        ("EVENT_CHAINING", event_code::CHAINING as i64),
        ("EVENT_ATTACK_ANNOUNCE", event_code::ATTACK_ANNOUNCE as i64),
        ("EVENT_BATTLE_DESTROYED", event_code::BATTLE_DESTROYED as i64),
        ("EVENT_BATTLE_DAMAGE", event_code::BATTLE_DAMAGE as i64),
        ("EVENT_DRAW", event_code::DRAW as i64),
        ("EVENT_DAMAGE", event_code::DAMAGE as i64),
        ("EVENT_PHASE", event_code::PHASE as i64),
        ("EVENT_PHASE_START", event_code::PHASE_START as i64),
        // Phases
        ("PHASE_DRAW", Phase::Draw.code() as i64),
        ("PHASE_STANDBY", Phase::Standby.code() as i64),
        ("PHASE_MAIN1", Phase::Main1.code() as i64),
        ("PHASE_BATTLE_START", Phase::BattleStart.code() as i64),
        ("PHASE_BATTLE_STEP", Phase::BattleStep.code() as i64),
        ("PHASE_DAMAGE", Phase::Damage.code() as i64),
        ("PHASE_DAMAGE_CAL", Phase::DamageCal.code() as i64),
        ("PHASE_BATTLE", Phase::BattleEnd.code() as i64),
        ("PHASE_MAIN2", Phase::Main2.code() as i64),
        ("PHASE_END", Phase::End.code() as i64),
        // Resets
        ("RESET_EVENT", ResetFlag::EVENT.bits() as i64),
        ("RESET_PHASE", ResetFlag::PHASE.bits() as i64),
        ("RESET_LEAVE", ResetFlag::LEAVE.bits() as i64),
        ("RESET_TOGRAVE", ResetFlag::TOGRAVE.bits() as i64),
        ("RESET_STANDARD", ResetFlag::STANDARD.bits() as i64),
        ("RESET_SELF_TURN", ResetFlag::SELF_TURN.bits() as i64),
        ("RESET_OPPO_TURN", ResetFlag::OPPO_TURN.bits() as i64),
        // Summon types
        ("SUMMON_TYPE_NORMAL", SummonType::NORMAL.bits() as i64),
        ("SUMMON_TYPE_ADVANCE", SummonType::ADVANCE.bits() as i64),
        ("SUMMON_TYPE_FLIP", SummonType::FLIP.bits() as i64),
        ("SUMMON_TYPE_SPECIAL", SummonType::SPECIAL.bits() as i64),
        ("SUMMON_TYPE_FUSION", SummonType::FUSION.bits() as i64),
        ("SUMMON_TYPE_RITUAL", SummonType::RITUAL.bits() as i64),
        ("SUMMON_TYPE_SYNCHRO", SummonType::SYNCHRO.bits() as i64),
        ("SUMMON_TYPE_XYZ", SummonType::XYZ.bits() as i64),
        ("SUMMON_TYPE_PENDULUM", SummonType::PENDULUM.bits() as i64),
        ("SUMMON_TYPE_LINK", SummonType::LINK.bits() as i64),
        // Status
        ("STATUS_PROC_COMPLETE", CardStatus::PROC_COMPLETE.bits() as i64),
        ("STATUS_DISABLED", CardStatus::DISABLED.bits() as i64),
        ("STATUS_SUMMON_TURN", CardStatus::SUMMON_TURN.bits() as i64),
        ("STATUS_SET_TURN", CardStatus::SET_TURN.bits() as i64),
        // Count Limit Flags
        ("EFFECT_COUNT_CODE_OATH", 0x10000000),
        ("EFFECT_COUNT_CODE_DUEL", 0x20000000),
        ("EFFECT_COUNT_CODE_CHAIN", 0x40000000),
        // Misc / Timings / ChainInfo / Hints
        ("TIMING_END_PHASE", 0x10),
        ("TIMING_MAIN_END", 0x8),
        ("TIMING_BATTLE_START", 0x20),
        ("TIMING_SUMMON", 0x80),
        ("TIMING_SPSUMMON", 0x100),
        ("CHAININFO_TARGET_CARDS", 0x08),
        ("CHAININFO_TARGET_PLAYER", 0x20),
        ("CHAININFO_TARGET_PARAM", 0x40),
        ("HINT_SELECTMSG", 3),
        ("HINTMSG_DESTROY", 502),
        ("HINTMSG_ATOHAND", 506),
        ("HINTMSG_SPSUMMON", 509),
        ("HINTMSG_TOGRAVE", 504),
        ("HINTMSG_REMOVE", 503),
        ("PLAYER_ALL", 2),
        ("PLAYER_NONE", 3),
        ("DUEL_ATTACK_FIRST_TURN", 0x1),
        ("DUEL_SIMPLE_AI", 0x10),
    ];

    for &(k, v) in constants {
        vm.set_global(k, LuaVal::Int(v));
    }
}

/// Dispatch all `Duel.*`, `Card.*`, `Effect.*`, `Group.*`, `Debug.*`, and `Auxiliary.*` native calls!
pub fn call_ocg_native(
    vm: &mut LuaVm,
    full_name: &str,
    args: Vec<LuaVal>,
    engine: &mut Option<&mut DuelEngine>,
) -> Result<Vec<LuaVal>, LuaError> {
    if let Some(m) = full_name.strip_prefix("Effect.") {
        return call_effect_api(vm, m, args, engine);
    }
    if let Some(m) = full_name.strip_prefix("Card.") {
        return call_card_api(vm, m, args, engine);
    }
    if let Some(m) = full_name.strip_prefix("Duel.") {
        return call_duel_api(vm, m, args, engine);
    }
    if let Some(m) = full_name.strip_prefix("Group.") {
        return call_group_api(vm, m, args, engine);
    }
    if let Some(m) = full_name.strip_prefix("Debug.") {
        return call_debug_api(vm, m, args, engine);
    }
    if let Some(m) = full_name.strip_prefix("Auxiliary.") {
        return call_aux_api(vm, m, args);
    }
    Ok(vec![LuaVal::Nil])
}

fn get_effect_mut<'a>(
    vm: &'a mut LuaVm,
    engine: &'a mut Option<&mut DuelEngine>,
    eid: EffectId,
) -> Option<&'a mut Effect> {
    if vm.draft_effects.contains_key(&eid) {
        return vm.draft_effects.get_mut(&eid);
    }
    let resolved = vm.effect_id_aliases.get(&eid).copied().unwrap_or(eid);
    engine.as_deref_mut().and_then(|e| e.state.effects.get_mut(&resolved))
}

fn get_effect_ref<'a>(
    vm: &'a LuaVm,
    engine: &'a Option<&mut DuelEngine>,
    eid: EffectId,
) -> Option<&'a Effect> {
    if let Some(e) = vm.draft_effects.get(&eid) {
        return Some(e);
    }
    let resolved = vm.effect_id_aliases.get(&eid).copied().unwrap_or(eid);
    engine.as_deref().and_then(|e| e.state.effects.get(&resolved))
}

fn call_effect_api(
    vm: &mut LuaVm,
    method: &str,
    args: Vec<LuaVal>,
    engine: &mut Option<&mut DuelEngine>,
) -> Result<Vec<LuaVal>, LuaError> {
    match method {
        "CreateEffect" | "GlobalEffect" => {
            let owner_card = match args.first() {
                Some(LuaVal::Card(cid)) => Some(*cid),
                _ => None,
            };
            let owner_player = owner_card
                .and_then(|cid| engine.as_deref().and_then(|e| e.card(cid)).map(|c| c.controller))
                .unwrap_or(0);
            let eid = vm.next_draft_effect_id;
            vm.next_draft_effect_id += 1;
            vm.draft_effects.insert(eid, Effect::new(eid, owner_card, owner_player, eid as u64));
            Ok(vec![LuaVal::Effect(eid)])
        }
        "Clone" => {
            let Some(LuaVal::Effect(src_id)) = args.first() else {
                return Ok(vec![LuaVal::Nil]);
            };
            let Some(mut cloned) = get_effect_ref(vm, engine, *src_id).cloned() else {
                return Ok(vec![LuaVal::Nil]);
            };
            let eid = vm.next_draft_effect_id;
            vm.next_draft_effect_id += 1;
            cloned.id = eid;
            vm.draft_effects.insert(eid, cloned);
            Ok(vec![LuaVal::Effect(eid)])
        }
        "SetType" => {
            if let Some(LuaVal::Effect(eid)) = args.first() {
                let bits = args.get(1).map(|v| v.as_u32()).unwrap_or(0);
                if let Some(eff) = get_effect_mut(vm, engine, *eid) {
                    eff.effect_type = EffectType::from_bits_truncate(bits);
                }
            }
            Ok(vec![])
        }
        "SetCode" => {
            if let Some(LuaVal::Effect(eid)) = args.first() {
                let code = args.get(1).map(|v| v.as_u32()).unwrap_or(0);
                if let Some(eff) = get_effect_mut(vm, engine, *eid) {
                    eff.code = code;
                }
            }
            Ok(vec![])
        }
        "SetCategory" => {
            if let Some(LuaVal::Effect(eid)) = args.first() {
                let cat = args.get(1).map(|v| v.as_u64()).unwrap_or(0);
                if let Some(eff) = get_effect_mut(vm, engine, *eid) {
                    eff.category = Category::from_bits_truncate(cat);
                }
            }
            Ok(vec![])
        }
        "SetProperty" => {
            if let Some(LuaVal::Effect(eid)) = args.first() {
                let p1 = args.get(1).map(|v| v.as_u64()).unwrap_or(0);
                if let Some(eff) = get_effect_mut(vm, engine, *eid) {
                    eff.flags = EffectFlag::from_bits_truncate(p1);
                }
            }
            Ok(vec![])
        }
        "SetRange" => {
            if let Some(LuaVal::Effect(eid)) = args.first() {
                let r = args.get(1).map(|v| v.as_u32()).unwrap_or(0);
                if let Some(eff) = get_effect_mut(vm, engine, *eid) {
                    eff.range = Location::from_bits_truncate(r);
                }
            }
            Ok(vec![])
        }
        "SetTargetRange" => {
            if let Some(LuaVal::Effect(eid)) = args.first() {
                let s = args.get(1).map(|v| v.as_u32()).unwrap_or(0);
                let o = args.get(2).map(|v| v.as_u32()).unwrap_or(0);
                if let Some(eff) = get_effect_mut(vm, engine, *eid) {
                    eff.target_range = (s, o);
                }
            }
            Ok(vec![])
        }
        "SetCountLimit" => {
            if let Some(LuaVal::Effect(eid)) = args.first() {
                let max_count = args.get(1).map(|v| v.as_u32()).unwrap_or(1);
                let raw_code = args.get(2).map(|v| v.as_u32()).unwrap_or(0);
                let is_oath = (raw_code & 0x1000_0000) != 0;
                let is_duel = (raw_code & 0x2000_0000) != 0;
                let is_chain = (raw_code & 0x4000_0000) != 0;
                let code = raw_code & 0x0FFF_FFFF;
                if let Some(eff) = get_effect_mut(vm, engine, *eid) {
                    eff.count_limit = Some(CountLimit {
                        max_count,
                        code,
                        is_oath,
                        is_duel,
                        is_chain,
                        used_this_turn: 0,
                        used_this_chain: 0,
                    });
                }
            }
            Ok(vec![])
        }
        "SetCondition" => {
            if let (Some(LuaVal::Effect(eid)), Some(LuaVal::Closure(fid))) = (args.first(), args.get(1)) {
                if let Some(eff) = get_effect_mut(vm, engine, *eid) {
                    eff.condition = EffectCallback::LuaFunc(*fid);
                }
            }
            Ok(vec![])
        }
        "SetCost" => {
            if let (Some(LuaVal::Effect(eid)), Some(LuaVal::Closure(fid))) = (args.first(), args.get(1)) {
                if let Some(eff) = get_effect_mut(vm, engine, *eid) {
                    eff.cost = EffectCallback::LuaFunc(*fid);
                }
            }
            Ok(vec![])
        }
        "SetTarget" => {
            if let (Some(LuaVal::Effect(eid)), Some(LuaVal::Closure(fid))) = (args.first(), args.get(1)) {
                if let Some(eff) = get_effect_mut(vm, engine, *eid) {
                    eff.target = EffectCallback::LuaFunc(*fid);
                }
            }
            Ok(vec![])
        }
        "SetOperation" => {
            if let (Some(LuaVal::Effect(eid)), Some(LuaVal::Closure(fid))) = (args.first(), args.get(1)) {
                if let Some(eff) = get_effect_mut(vm, engine, *eid) {
                    eff.operation = EffectCallback::LuaFunc(*fid);
                }
            }
            Ok(vec![])
        }
        "SetValue" => {
            if let Some(LuaVal::Effect(eid)) = args.first() {
                let val = match args.get(1) {
                    Some(LuaVal::Closure(fid)) => EffectValue::Callback(EffectCallback::LuaFunc(*fid)),
                    Some(v) => EffectValue::Int(v.as_int()),
                    None => EffectValue::None,
                };
                if let Some(eff) = get_effect_mut(vm, engine, *eid) {
                    eff.value = val;
                }
            }
            Ok(vec![])
        }
        "SetReset" => {
            if let Some(LuaVal::Effect(eid)) = args.first() {
                let raw = args.get(1).map(|v| v.as_u32()).unwrap_or(0);
                let count = args.get(2).map(|v| v.as_u32() as u8).unwrap_or(1);
                if let Some(eff) = get_effect_mut(vm, engine, *eid) {
                    eff.reset_flags = ResetFlag::from_bits_truncate(raw);
                    eff.reset_count = count;
                    eff.reset_phase = Phase::from_code(raw & 0x3FF);
                }
            }
            Ok(vec![])
        }
        "SetDescription" => {
            if let Some(LuaVal::Effect(eid)) = args.first() {
                let d = args.get(1).map(|v| v.as_u32()).unwrap_or(0);
                if let Some(eff) = get_effect_mut(vm, engine, *eid) {
                    eff.description = d;
                }
            }
            Ok(vec![])
        }
        "SetHintTiming" => {
            if let Some(LuaVal::Effect(eid)) = args.first() {
                let t1 = args.get(1).map(|v| v.as_u32()).unwrap_or(0);
                let t2 = args.get(2).map(|v| v.as_u32()).unwrap_or(t1);
                if let Some(eff) = get_effect_mut(vm, engine, *eid) {
                    eff.hint_timing = (t1, t2);
                }
            }
            Ok(vec![])
        }
        "SetLabel" => {
            if let Some(LuaVal::Effect(eid)) = args.first() {
                let l = args.get(1).map(|v| v.as_int()).unwrap_or(0);
                if let Some(eff) = get_effect_mut(vm, engine, *eid) {
                    eff.label = l;
                }
            }
            Ok(vec![])
        }
        "GetLabel" => {
            let l = match args.first() {
                Some(LuaVal::Effect(eid)) => get_effect_ref(vm, engine, *eid).map(|e| e.label).unwrap_or(0),
                _ => 0,
            };
            Ok(vec![LuaVal::Int(l)])
        }
        "SetLabelObject" => {
            if let Some(LuaVal::Effect(eid)) = args.first() {
                let obj = match args.get(1) {
                    Some(LuaVal::Card(cid)) => Some(LabelObject::Card(*cid)),
                    Some(LuaVal::Effect(oe)) => Some(LabelObject::Effect(*oe)),
                    Some(LuaVal::Int(i)) => Some(LabelObject::Int(*i)),
                    Some(LuaVal::Str(s)) => Some(LabelObject::Str(s.clone())),
                    _ => None,
                };
                if let Some(eff) = get_effect_mut(vm, engine, *eid) {
                    eff.label_object = obj;
                }
            }
            Ok(vec![])
        }
        "GetLabelObject" => {
            let v = match args.first() {
                Some(LuaVal::Effect(eid)) => match get_effect_ref(vm, engine, *eid).and_then(|e| e.label_object.clone()) {
                    Some(LabelObject::Card(cid)) => LuaVal::Card(cid),
                    Some(LabelObject::Effect(oe)) => LuaVal::Effect(oe),
                    Some(LabelObject::Int(i)) => LuaVal::Int(i),
                    Some(LabelObject::Str(s)) => LuaVal::Str(s),
                    _ => LuaVal::Nil,
                },
                _ => LuaVal::Nil,
            };
            Ok(vec![v])
        }
        "GetHandler" => {
            let v = match args.first() {
                Some(LuaVal::Effect(eid)) => get_effect_ref(vm, engine, *eid)
                    .and_then(|e| e.handler_card)
                    .map(LuaVal::Card)
                    .unwrap_or(LuaVal::Nil),
                _ => LuaVal::Nil,
            };
            Ok(vec![v])
        }
        "GetHandlerPlayer" | "GetOwnerPlayer" => {
            let p = match args.first() {
                Some(LuaVal::Effect(eid)) => get_effect_ref(vm, engine, *eid).map(|e| e.owner_player as i64).unwrap_or(0),
                _ => 0,
            };
            Ok(vec![LuaVal::Int(p)])
        }
        "GetOwner" => {
            let v = match args.first() {
                Some(LuaVal::Effect(eid)) => get_effect_ref(vm, engine, *eid)
                    .and_then(|e| e.owner_card)
                    .map(LuaVal::Card)
                    .unwrap_or(LuaVal::Nil),
                _ => LuaVal::Nil,
            };
            Ok(vec![v])
        }
        "GetCode" => {
            let c = match args.first() {
                Some(LuaVal::Effect(eid)) => get_effect_ref(vm, engine, *eid).map(|e| e.code as i64).unwrap_or(0),
                _ => 0,
            };
            Ok(vec![LuaVal::Int(c)])
        }
        "Reset" => {
            if let Some(LuaVal::Effect(eid)) = args.first() {
                if let Some(eff) = get_effect_mut(vm, engine, *eid) {
                    eff.active = false;
                }
            }
            Ok(vec![])
        }
        _ => Ok(vec![LuaVal::Nil]),
    }
}

fn call_card_api(
    vm: &mut LuaVm,
    method: &str,
    args: Vec<LuaVal>,
    engine: &mut Option<&mut DuelEngine>,
) -> Result<Vec<LuaVal>, LuaError> {
    let Some(LuaVal::Card(cid)) = args.first().cloned() else {
        return Ok(vec![LuaVal::Nil]);
    };
    let Some(eng) = engine.as_deref_mut() else {
        return Ok(vec![LuaVal::Nil]);
    };

    if method == "RegisterEffect" {
        if let Some(LuaVal::Effect(draft_id)) = args.get(1) {
            if let Some(mut eff) = vm.draft_effects.remove(draft_id) {
                eff.handler_card = Some(cid);
                if let Some(c) = eng.card(cid) {
                    eff.owner_player = c.controller;
                }
                let mut noop = NoopScriptHost;
                let real_id = eng.register_effect(eff, &mut noop);
                vm.effect_id_aliases.insert(*draft_id, real_id);
                return Ok(vec![LuaVal::Int(real_id as i64)]);
            }
        }
        return Ok(vec![LuaVal::Int(0)]);
    }

    if method == "AddCounter" {
        let ct = args.get(1).map(|v| v.as_u32() as u16).unwrap_or(0);
        let cnt = args.get(2).map(|v| v.as_u32()).unwrap_or(1);
        if let Some(c) = eng.card_mut(cid) {
            c.add_counter(ct, cnt);
        }
        eng.emit(DuelMessage::AddCounter { card_id: cid, counter_type: ct, count: cnt });
        return Ok(vec![LuaVal::Bool(true)]);
    }

    if method == "RemoveCounter" {
        let ct = args.get(2).map(|v| v.as_u32() as u16).unwrap_or(0);
        let cnt = args.get(3).map(|v| v.as_u32()).unwrap_or(1);
        let ok = eng.card_mut(cid).map(|c| c.remove_counter(ct, cnt)).unwrap_or(false);
        if ok {
            eng.emit(DuelMessage::RemoveCounter { card_id: cid, counter_type: ct, count: cnt });
        }
        return Ok(vec![LuaVal::Bool(ok)]);
    }

    if method == "RemoveOverlayCard" {
        let tp = args.get(1).map(|v| v.as_u32() as PlayerId).unwrap_or(0);
        let min = args.get(2).map(|v| v.as_u32() as usize).unwrap_or(1);
        let reason = Reason::from_bits_truncate(args.get(4).map(|v| v.as_u32()).unwrap_or(Reason::COST.bits()));
        let mut noop = NoopScriptHost;
        let ok = eng.detach_overlay(cid, min, reason, tp, &mut noop).is_ok();
        return Ok(vec![LuaVal::Bool(ok)]);
    }

    if method == "RegisterFlagEffect" {
        let code = args.get(1).map(|v| v.as_u32()).unwrap_or(0);
        let label = args.get(4).map(|v| v.as_int()).unwrap_or(1);
        if let Some(c) = eng.card_mut(cid) {
            c.flag_effects.insert(code, label);
        }
        return Ok(vec![LuaVal::Bool(true)]);
    }

    if method == "GetMetatable" {
        let mt = vm.card_metatable(cid);
        return Ok(vec![LuaVal::Table(mt)]);
    }

    if method == "EnableReviveLimit" {
        if let Some(c) = eng.card_mut(cid) {
            c.status.insert(CardStatus::REVIVE_LIMIT);
        }
        return Ok(vec![]);
    }

    if method == "SetStatus" {
        let s = CardStatus::from_bits_truncate(args.get(1).map(|v| v.as_u32()).unwrap_or(0));
        let enable = args.get(2).map(|v| v.is_truthy()).unwrap_or(true);
        if let Some(c) = eng.card_mut(cid) {
            if enable { c.status.insert(s); } else { c.status.remove(s); }
        }
        return Ok(vec![]);
    }

    if matches!(
        method,
        "EnableCounterPermit"
            | "SetCounterLimit"
            | "SetUniqueOnField"
            | "SetSPSummonOnce"
            | "EnableUnsummonable"
            | "AddMonsterAttribute"
            | "EnablePendulumAttribute"
            | "SetFlagEffectLabel"
    ) {
        return Ok(vec![]);
    }

    if method == "ResetFlagEffect" {
        let code = args.get(1).map(|v| v.as_u32()).unwrap_or(0);
        if let Some(c) = eng.card_mut(cid) {
            c.flag_effects.remove(&code);
        }
        return Ok(vec![]);
    }

    let Some(c) = eng.card(cid).cloned() else {
        return Ok(vec![LuaVal::Nil]);
    };

    let val = match method {
        "GetCode" => LuaVal::Int(c.computed.code as i64),
        "GetOriginalCode" | "GetOriginalCodeRule" => LuaVal::Int(c.data.code as i64),
        "IsOriginalCodeRule" => {
            let ok = args[1..].iter().any(|v| v.as_u32() == c.data.code || v.as_u32() == c.data.effective_alias());
            LuaVal::Bool(ok)
        }
        "GetFlagEffectLabel" => {
            let code = args.get(1).map(|v| v.as_u32()).unwrap_or(0);
            c.flag_effects.get(&code).copied().map(LuaVal::Int).unwrap_or(LuaVal::Nil)
        }
        "ListsCode" | "ListsArchetype" | "IsSummonCode" => LuaVal::Bool(false),
        "GetType" => LuaVal::Int(c.computed.card_type.bits() as i64),
        "GetAttribute" => LuaVal::Int(c.computed.attribute.bits() as i64),
        "GetRace" => LuaVal::Int(c.computed.race.bits() as i64),
        "GetLevel" => LuaVal::Int(c.computed.level as i64),
        "GetRank" => LuaVal::Int(c.computed.rank as i64),
        "GetLink" => LuaVal::Int(c.computed.link as i64),
        "GetAttack" => LuaVal::Int(c.current_atk() as i64),
        "GetDefense" => LuaVal::Int(c.current_def() as i64),
        "GetBaseAttack" => LuaVal::Int(c.computed.base_atk as i64),
        "GetBaseDefense" => LuaVal::Int(c.computed.base_def as i64),
        "GetLocation" => LuaVal::Int(c.location.bits() as i64),
        "GetSequence" => LuaVal::Int(c.sequence as i64),
        "GetControler" => LuaVal::Int(c.controller as i64),
        "GetOwner" => LuaVal::Int(c.owner as i64),
        "GetPosition" => LuaVal::Int(c.position.bits() as i64),
        "GetLeftScale" => LuaVal::Int(c.computed.lscale as i64),
        "GetRightScale" => LuaVal::Int(c.computed.rscale as i64),
        "IsFaceup" => LuaVal::Bool(c.is_faceup()),
        "IsFacedown" => LuaVal::Bool(c.is_facedown()),
        "IsAttackPos" => LuaVal::Bool(c.is_attack_pos()),
        "IsDefensePos" => LuaVal::Bool(c.is_defense_pos()),
        "IsOnField" => LuaVal::Bool(c.is_on_field()),
        "IsPublic" => LuaVal::Bool(c.is_public()),
        "IsDisabled" => LuaVal::Bool(c.is_disabled()),
        "IsMonster" => LuaVal::Bool(c.data.is_monster()),
        "IsSpell" => LuaVal::Bool(c.data.is_spell()),
        "IsTrap" => LuaVal::Bool(c.data.is_trap()),
        "IsType" => {
            let t = CardType::from_bits_truncate(args.get(1).map(|v| v.as_u32()).unwrap_or(0));
            LuaVal::Bool(c.computed.card_type.intersects(t))
        }
        "IsAttribute" => {
            let a = Attribute::from_bits_truncate(args.get(1).map(|v| v.as_u32()).unwrap_or(0));
            LuaVal::Bool(c.computed.attribute.intersects(a))
        }
        "IsRace" => {
            let r = Race::from_bits_truncate(args.get(1).map(|v| v.as_u64()).unwrap_or(0));
            LuaVal::Bool(c.computed.race.intersects(r))
        }
        "IsSetCard" => {
            let sc = args.get(1).map(|v| v.as_u32() as u16).unwrap_or(0);
            LuaVal::Bool(!c.is_facedown_banished() && c.data.is_set_card(sc))
        }
        "IsCode" => {
            let ok = args[1..].iter().any(|v| c.has_code(v.as_u32()));
            LuaVal::Bool(ok)
        }
        "IsLocation" => {
            let l = Location::from_bits_truncate(args.get(1).map(|v| v.as_u32()).unwrap_or(0));
            LuaVal::Bool(c.location.intersects(l))
        }
        "IsPreviousLocation" => {
            let l = Location::from_bits_truncate(args.get(1).map(|v| v.as_u32()).unwrap_or(0));
            LuaVal::Bool(c.prev_location.intersects(l))
        }
        "IsControler" => LuaVal::Bool(c.controller == args.get(1).map(|v| v.as_u32() as PlayerId).unwrap_or(255)),
        "IsPosition" => {
            let p = Position::from_bits_truncate(args.get(1).map(|v| v.as_u32()).unwrap_or(0));
            LuaVal::Bool(c.position.intersects(p))
        }
        "IsReason" => {
            let r = Reason::from_bits_truncate(args.get(1).map(|v| v.as_u32()).unwrap_or(0));
            LuaVal::Bool(c.reason.intersects(r))
        }
        "IsStatus" => {
            let s = CardStatus::from_bits_truncate(args.get(1).map(|v| v.as_u32()).unwrap_or(0));
            LuaVal::Bool(c.status.intersects(s))
        }
        "IsSummonType" => {
            let s = SummonType::from_bits_truncate(args.get(1).map(|v| v.as_u32()).unwrap_or(0));
            LuaVal::Bool(c.summon_type.intersects(s))
        }
        "IsAbleToHand" => {
            let team = eng.team_of(c.owner);
            let blocked = c.location == Location::DECK && eng.state.floodgates.cannot_to_hand_from_deck[team];
            LuaVal::Bool(!blocked)
        }
        "IsAbleToGrave" | "IsAbleToDeck" | "IsReleasable" | "IsDiscardable" => LuaVal::Bool(true),
        "IsAbleToRemove" => {
            let team = eng.team_of(c.owner);
            LuaVal::Bool(!eng.state.floodgates.cannot_remove[team])
        }
        "IsCanBeSpecialSummoned" => {
            let tp = args.get(3).map(|v| v.as_u32() as PlayerId).unwrap_or(c.owner);
            let team = eng.team_of(tp);
            let ignore_revive = args.get(5).map(|v| v.is_truthy()).unwrap_or(false);
            let revive_ok = ignore_revive
                || !matches!(c.location, Location::GRAVE | Location::REMOVED)
                || !c.requires_proper_summon()
                || c.is_properly_summoned();
            LuaVal::Bool(c.data.is_monster() && !eng.state.floodgates.cannot_special_summon[team] && revive_ok)
        }
        "IsCanBeEffectTarget" => LuaVal::Bool(!c.computed.untargetable_effect),
        "IsDestructable" => LuaVal::Bool(!c.computed.indestructable_effect),
        "IsRelateToEffect" => LuaVal::Bool(c.is_on_field() || matches!(c.location, Location::GRAVE | Location::REMOVED | Location::HAND)),
        "IsRelateToBattle" => LuaVal::Bool(eng.state.battle.attacker == Some(cid) || eng.state.battle.defender == Some(cid)),
        "GetOverlayCount" => LuaVal::Int(c.overlay_materials.len() as i64),
        "GetOverlayGroup" => LuaVal::Group(vm.alloc_group(c.overlay_materials.clone())),
        "CheckRemoveOverlayCard" => {
            let cnt = args.get(2).map(|v| v.as_u32() as usize).unwrap_or(1);
            LuaVal::Bool(c.overlay_materials.len() >= cnt)
        }
        "GetCounter" => {
            let ct = args.get(1).map(|v| v.as_u32() as u16).unwrap_or(0);
            LuaVal::Int(c.counter_count(ct) as i64)
        }
        "GetEquipTarget" => c.equip_target.map(LuaVal::Card).unwrap_or(LuaVal::Nil),
        "GetEquipGroup" => LuaVal::Group(vm.alloc_group(c.equipped_cards.clone())),
        "GetLinkedGroup" => LuaVal::Group(vm.alloc_group(eng.linked_cards(cid))),
        "GetColumnGroup" => LuaVal::Group(vm.alloc_group(eng.column_cards(cid))),
        "GetFlagEffect" => {
            let code = args.get(1).map(|v| v.as_u32()).unwrap_or(0);
            LuaVal::Int(if c.flag_effects.contains_key(&code) { 1 } else { 0 })
        }
        _ => LuaVal::Nil,
    };
    Ok(vec![val])
}

fn extract_cards(vm: &LuaVm, val: Option<&LuaVal>) -> Vec<CardId> {
    match val {
        Some(LuaVal::Card(cid)) => vec![*cid],
        Some(LuaVal::Group(gid)) => vm.groups.get(gid).map(|g| g.cards.clone()).unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn call_duel_api(
    vm: &mut LuaVm,
    method: &str,
    args: Vec<LuaVal>,
    engine: &mut Option<&mut DuelEngine>,
) -> Result<Vec<LuaVal>, LuaError> {
    // Methods that work both with and without an active DuelEngine (e.g., during core library load)
    match method {
        "LoadScript" | "ImportScript" => {
            let fname = args.first().map(|v| v.as_str()).unwrap_or_default();
            if fname.is_empty() || vm.loaded_helper_scripts.contains(&fname) {
                return Ok(vec![LuaVal::Bool(true)]);
            }
            for dir in vm.script_search_dirs.clone() {
                let p = dir.join(&fname);
                if p.is_file() {
                    if let Ok(src) = std::fs::read_to_string(&p) {
                        vm.loaded_helper_scripts.insert(fname.clone());
                        let eng_opt = engine.as_deref_mut();
                        let _ = vm.exec(&src, eng_opt);
                        return Ok(vec![LuaVal::Bool(true)]);
                    }
                }
            }
            return Ok(vec![LuaVal::Bool(false)]);
        }
        "LoadCardScript" | "LoadCardScriptAlias" => {
            let fname = if method == "LoadCardScriptAlias" {
                format!("c{}.lua", args.first().map(|v| v.as_u32()).unwrap_or(0))
            } else {
                args.first().map(|v| v.as_str()).unwrap_or_default()
            };
            for dir in vm.script_search_dirs.clone() {
                let p = dir.join(&fname);
                if p.is_file() {
                    if let Ok(src) = std::fs::read_to_string(&p) {
                        let eng_opt = engine.as_deref_mut();
                        let _ = vm.exec(&src, eng_opt);
                        return Ok(vec![LuaVal::Bool(true)]);
                    }
                }
            }
            return Ok(vec![LuaVal::Bool(false)]);
        }
        "EnableUnofficialProc" | "EnableGlobalFlag" | "AddCustomActivityCounter" => {
            return Ok(vec![]);
        }
        "GetCustomActivityCount" | "GetFlagEffect" | "GetFlagEffectLabel" => {
            return Ok(vec![LuaVal::Int(0)]);
        }
        "RegisterFlagEffect" | "ResetFlagEffect" | "SetFlagEffectLabel" => {
            return Ok(vec![LuaVal::Bool(true)]);
        }
        "GetReasonEffect" => {
            return Ok(vec![LuaVal::Nil]);
        }
        "IsDuelType" => {
            let mask = args.first().map(|v| v.as_u64()).unwrap_or(0);
            let res = engine
                .as_deref()
                .map(|e| (e.state.config.flags.bits() & mask) != 0)
                .unwrap_or(false);
            return Ok(vec![LuaVal::Bool(res)]);
        }
        "GetMasterRule" => {
            let mr = engine.as_deref().map(|e| e.state.config.master_rule as i64).unwrap_or(5);
            return Ok(vec![LuaVal::Int(mr)]);
        }
        "GetEnvironment" => {
            return Ok(vec![LuaVal::Int(0), LuaVal::Int(0)]);
        }
        "GetOperationInfo" => {
            return Ok(vec![LuaVal::Bool(false)]);
        }
        "GetChainMaterial" => {
            return Ok(vec![LuaVal::Nil]);
        }
        "CheckEvent" => {
            let ev = args.first().map(|v| v.as_u32()).unwrap_or(0);
            let ok = engine
                .as_deref()
                .map(|e| e.state.pending_events.iter().any(|x| x.code == ev))
                .unwrap_or(false);
            return Ok(vec![LuaVal::Bool(ok)]);
        }
        "GetTurnID" => {
            let t = engine.as_deref().map(|e| e.state.turn_count as i64).unwrap_or(1);
            return Ok(vec![LuaVal::Int(t)]);
        }
        "RegisterEffect" if engine.is_none() => {
            return Ok(vec![LuaVal::Int(0)]);
        }
        _ => {}
    }

    let Some(eng) = engine.as_deref_mut() else {
        return Ok(vec![LuaVal::Nil]);
    };
    let mut noop = NoopScriptHost;

    match method {
        "Draw" => {
            let p = args.first().map(|v| v.as_u32() as PlayerId).unwrap_or(0);
            let n = args.get(1).map(|v| v.as_u32() as usize).unwrap_or(1);
            let r = Reason::from_bits_truncate(args.get(2).map(|v| v.as_u32()).unwrap_or(Reason::EFFECT.bits()));
            let drawn = eng.draw_cards(p, n, r, &mut noop);
            vm.operated_group = drawn.clone();
            Ok(vec![LuaVal::Int(drawn.len() as i64)])
        }
        "IsPlayerCanDraw" => {
            let p = args.first().map(|v| v.as_u32() as PlayerId).unwrap_or(0);
            let n = args.get(1).map(|v| v.as_u32() as usize).unwrap_or(1);
            let team = eng.team_of(p);
            let ok = !eng.state.floodgates.cannot_draw[team] && eng.cards_in(p, Location::DECK).len() >= n;
            Ok(vec![LuaVal::Bool(ok)])
        }
        "IsPlayerCanSpecialSummon" | "IsPlayerCanSpecialSummonMonster" => {
            let p = args.first().map(|v| v.as_u32() as PlayerId).unwrap_or(0);
            let team = eng.team_of(p);
            Ok(vec![LuaVal::Bool(!eng.state.floodgates.cannot_special_summon[team])])
        }
        "Damage" => {
            let p = args.first().map(|v| v.as_u32() as PlayerId).unwrap_or(0);
            let amt = args.get(1).map(|v| v.as_int() as i32).unwrap_or(0);
            let r = Reason::from_bits_truncate(args.get(2).map(|v| v.as_u32()).unwrap_or(Reason::EFFECT.bits()));
            eng.inflict_damage(p, amt, false, r, &mut noop);
            Ok(vec![LuaVal::Int(amt as i64)])
        }
        "Recover" => {
            let p = args.first().map(|v| v.as_u32() as PlayerId).unwrap_or(0);
            let amt = args.get(1).map(|v| v.as_int() as i32).unwrap_or(0);
            let r = Reason::from_bits_truncate(args.get(2).map(|v| v.as_u32()).unwrap_or(Reason::EFFECT.bits()));
            eng.recover_lp(p, amt, r, &mut noop);
            Ok(vec![LuaVal::Int(amt as i64)])
        }
        "GetLP" => {
            let p = args.first().map(|v| v.as_u32() as PlayerId).unwrap_or(0);
            Ok(vec![LuaVal::Int(eng.get_lp(p) as i64)])
        }
        "SetLP" => {
            let p = args.first().map(|v| v.as_u32() as PlayerId).unwrap_or(0);
            let lp = args.get(1).map(|v| v.as_int() as i32).unwrap_or(0);
            eng.set_lp(p, lp);
            Ok(vec![])
        }
        "CheckLPCost" => {
            let p = args.first().map(|v| v.as_u32() as PlayerId).unwrap_or(0);
            let cost = args.get(1).map(|v| v.as_int() as i32).unwrap_or(0);
            Ok(vec![LuaVal::Bool(eng.get_lp(p) > cost)])
        }
        "PayLPCost" => {
            let p = args.first().map(|v| v.as_u32() as PlayerId).unwrap_or(0);
            let cost = args.get(1).map(|v| v.as_int() as i32).unwrap_or(0);
            Ok(vec![LuaVal::Bool(eng.pay_lp_cost(p, cost))])
        }
        "Destroy" => {
            let cards = extract_cards(vm, args.first());
            let r = Reason::from_bits_truncate(args.get(1).map(|v| v.as_u32()).unwrap_or(Reason::EFFECT.bits()));
            let tp = eng.state.turn_player;
            let destroyed = eng.destroy_cards(&cards, r, tp, None, &mut noop);
            vm.operated_group = destroyed.clone();
            Ok(vec![LuaVal::Int(destroyed.len() as i64)])
        }
        "SendtoGrave" => {
            let cards = extract_cards(vm, args.first());
            let r = Reason::from_bits_truncate(args.get(1).map(|v| v.as_u32()).unwrap_or(Reason::EFFECT.bits()));
            let mut moved = Vec::new();
            for cid in cards {
                if let Some(owner) = eng.card(cid).map(|c| c.owner) {
                    if eng.move_card(cid, owner, Location::GRAVE, None, Position::FACEUP_ATTACK, r, owner, None, &mut noop).unwrap_or(false) {
                        moved.push(cid);
                    }
                }
            }
            vm.operated_group = moved.clone();
            Ok(vec![LuaVal::Int(moved.len() as i64)])
        }
        "SendtoHand" => {
            let cards = extract_cards(vm, args.first());
            let target_p = args.get(1).and_then(|v| match v {
                LuaVal::Nil => None,
                x => Some(x.as_u32() as PlayerId),
            });
            let r = Reason::from_bits_truncate(args.get(2).map(|v| v.as_u32()).unwrap_or(Reason::EFFECT.bits()));
            let mut moved = Vec::new();
            for cid in cards {
                if let Some(owner) = eng.card(cid).map(|c| c.owner) {
                    let tp = target_p.unwrap_or(owner);
                    if eng.move_card(cid, tp, Location::HAND, None, Position::FACEUP_ATTACK, r, tp, None, &mut noop).unwrap_or(false) {
                        moved.push(cid);
                    }
                }
            }
            vm.operated_group = moved.clone();
            Ok(vec![LuaVal::Int(moved.len() as i64)])
        }
        "SendtoDeck" => {
            let cards = extract_cards(vm, args.first());
            let r = Reason::from_bits_truncate(args.get(3).map(|v| v.as_u32()).unwrap_or(Reason::EFFECT.bits()));
            let mut moved = Vec::new();
            for cid in cards {
                if let Some(owner) = eng.card(cid).map(|c| c.owner) {
                    if eng.move_card(cid, owner, Location::DECK, None, Position::FACEDOWN_DEFENSE, r, owner, None, &mut noop).unwrap_or(false) {
                        moved.push(cid);
                    }
                }
            }
            vm.operated_group = moved.clone();
            Ok(vec![LuaVal::Int(moved.len() as i64)])
        }
        "Remove" => {
            let cards = extract_cards(vm, args.first());
            let pos = Position::from_bits_truncate(args.get(1).map(|v| v.as_u32()).unwrap_or(Position::FACEUP_ATTACK.bits()));
            let r = Reason::from_bits_truncate(args.get(2).map(|v| v.as_u32()).unwrap_or(Reason::EFFECT.bits()));
            let mut moved = Vec::new();
            for cid in cards {
                if let Some(owner) = eng.card(cid).map(|c| c.owner) {
                    if eng.move_card(cid, owner, Location::REMOVED, None, pos, r, owner, None, &mut noop).unwrap_or(false) {
                        moved.push(cid);
                    }
                }
            }
            vm.operated_group = moved.clone();
            Ok(vec![LuaVal::Int(moved.len() as i64)])
        }
        "Release" => {
            let cards = extract_cards(vm, args.first());
            let r = Reason::from_bits_truncate(args.get(1).map(|v| v.as_u32()).unwrap_or(Reason::RELEASE.bits()));
            let mut moved = Vec::new();
            for cid in cards {
                if let Some(owner) = eng.card(cid).map(|c| c.owner) {
                    if eng.move_card(cid, owner, Location::GRAVE, None, Position::FACEUP_ATTACK, r | Reason::RELEASE, owner, None, &mut noop).unwrap_or(false) {
                        moved.push(cid);
                    }
                }
            }
            vm.operated_group = moved.clone();
            Ok(vec![LuaVal::Int(moved.len() as i64)])
        }
        "DiscardHand" => {
            let p = args.first().map(|v| v.as_u32() as PlayerId).unwrap_or(0);
            let min = args.get(2).map(|v| v.as_u32() as usize).unwrap_or(1);
            let r = Reason::from_bits_truncate(args.get(4).map(|v| v.as_u32()).unwrap_or(Reason::DISCARD.bits()));
            let hand = eng.cards_in(p, Location::HAND);
            let mut discarded = Vec::new();
            for cid in hand.into_iter().take(min) {
                if eng.move_card(cid, p, Location::GRAVE, None, Position::FACEUP_ATTACK, r | Reason::DISCARD, p, None, &mut noop).unwrap_or(false) {
                    discarded.push(cid);
                }
            }
            vm.operated_group = discarded.clone();
            Ok(vec![LuaVal::Int(discarded.len() as i64)])
        }
        "SpecialSummon" | "SpecialSummonStep" => {
            let cards = extract_cards(vm, args.first());
            let stype = SummonType::from_bits_truncate(args.get(1).map(|v| v.as_u32()).unwrap_or(SummonType::SPECIAL.bits()));
            let tp = args.get(2).map(|v| v.as_u32() as PlayerId).unwrap_or(0);
            let ignore_revive = args.get(5).map(|v| v.is_truthy()).unwrap_or(false);
            let pos = Position::from_bits_truncate(args.get(6).map(|v| v.as_u32()).unwrap_or(Position::FACEUP_ATTACK.bits()));
            let mut summoned = Vec::new();
            for cid in cards {
                if eng.special_summon(tp, cid, stype, pos, None, ignore_revive, false, &mut noop).is_ok() {
                    summoned.push(cid);
                }
            }
            vm.operated_group = summoned.clone();
            if method == "SpecialSummonStep" {
                Ok(vec![LuaVal::Bool(!summoned.is_empty())])
            } else {
                Ok(vec![LuaVal::Int(summoned.len() as i64)])
            }
        }
        "SpecialSummonComplete" => Ok(vec![LuaVal::Int(vm.operated_group.len() as i64)]),
        "ChangePosition" => {
            let cards = extract_cards(vm, args.first());
            let pos = Position::from_bits_truncate(args.get(1).map(|v| v.as_u32()).unwrap_or(Position::FACEUP_DEFENSE.bits()));
            let mut changed = 0i64;
            for cid in cards {
                // Official Rule: Tokens and Link Monsters cannot be flipped face-down!
                let can_change = eng.card(cid).map(|c| {
                    if pos.intersects(Position::FACEDOWN)
                        && (c.data.card_type.contains(CardType::TOKEN) || c.data.is_link())
                    {
                        false
                    } else if pos.intersects(Position::DEFENSE) && c.data.is_link() {
                        false
                    } else {
                        true
                    }
                }).unwrap_or(false);
                if !can_change {
                    continue;
                }
                if let Some(c) = eng.card_mut(cid) {
                    let prev = c.position;
                    c.prev_position = prev;
                    c.position = pos;
                    let code = c.data.code;
                    let ctrl = c.controller;
                    let loc = c.location;
                    let seq = c.sequence;
                    eng.emit(DuelMessage::PosChange { card_id: cid, code, controller: ctrl, location: loc, sequence: seq, prev_position: prev, new_position: pos });
                    changed += 1;
                }
            }
            eng.recalculate_effects(&mut noop);
            Ok(vec![LuaVal::Int(changed)])
        }
        "Equip" => {
            if let (Some(LuaVal::Card(eq_id)), Some(LuaVal::Card(target_id))) = (args.get(1), args.get(2)) {
                if let Some(eq) = eng.card_mut(*eq_id) {
                    eq.equip_target = Some(*target_id);
                }
                if let Some(t) = eng.card_mut(*target_id) {
                    if !t.equipped_cards.contains(eq_id) {
                        t.equipped_cards.push(*eq_id);
                    }
                }
                eng.emit(DuelMessage::Equip { equip_card: *eq_id, target_card: *target_id });
                eng.recalculate_effects(&mut noop);
                return Ok(vec![LuaVal::Bool(true)]);
            }
            Ok(vec![LuaVal::Bool(false)])
        }
        "GetLocationCount" => {
            let p = args.first().map(|v| v.as_u32() as PlayerId).unwrap_or(0);
            let loc = Location::from_bits_truncate(args.get(1).map(|v| v.as_u32()).unwrap_or(Location::MZONE.bits()));
            let count = if loc.contains(Location::MZONE) {
                eng.free_mmz_zones(p).len()
            } else if loc.contains(Location::SZONE) {
                eng.free_stz_zones(p).len()
            } else {
                0
            };
            Ok(vec![LuaVal::Int(count as i64)])
        }
        "GetLocationCountFromEx" => {
            let p = args.first().map(|v| v.as_u32() as PlayerId).unwrap_or(0);
            let count = if let Some(LuaVal::Card(cid)) = args.get(3) {
                eng.card(*cid).map(|c| eng.valid_extra_deck_zones(p, &c.data).len()).unwrap_or(1)
            } else {
                eng.free_mmz_zones(p).len().max(1)
            };
            Ok(vec![LuaVal::Int(count as i64)])
        }
        "GetMatchingGroup" | "GetMatchingGroupCount" | "IsExistingMatchingCard" | "IsExistingTarget" | "SelectMatchingCard" | "SelectTarget" | "GetFirstMatchingCard" => {
            let has_sel_player = matches!(method, "SelectMatchingCard" | "SelectTarget");
            let base_idx = if has_sel_player { 1 } else { 0 };
            let filter_fn = args.get(base_idx).cloned().unwrap_or(LuaVal::Nil);
            let tp = args.get(base_idx + 1).map(|v| v.as_u32() as PlayerId).unwrap_or(0);
            let s_loc = Location::from_bits_truncate(args.get(base_idx + 2).map(|v| v.as_u32()).unwrap_or(0));
            let o_loc = Location::from_bits_truncate(args.get(base_idx + 3).map(|v| v.as_u32()).unwrap_or(0));
            let count_min = if matches!(method, "IsExistingMatchingCard" | "IsExistingTarget" | "SelectMatchingCard" | "SelectTarget") {
                args.get(base_idx + 4).map(|v| v.as_u32() as usize).unwrap_or(1)
            } else {
                0
            };
            let exc_idx = if has_sel_player { base_idx + 6 } else if matches!(method, "IsExistingMatchingCard" | "IsExistingTarget") { base_idx + 5 } else { base_idx + 4 };
            let excluded = extract_cards(vm, args.get(exc_idx));
            let extra_args = if args.len() > exc_idx + 1 { args[(exc_idx + 1)..].to_vec() } else { Vec::new() };

            let mut candidates = Vec::new();
            if !s_loc.is_empty() {
                candidates.extend(eng.cards_in(tp, s_loc));
            }
            if !o_loc.is_empty() {
                let opp = eng.opponent_of(tp);
                candidates.extend(eng.cards_in(opp, o_loc));
            }
            candidates.retain(|cid| !excluded.contains(cid));

            let mut matched = Vec::new();
            for cid in candidates {
                if matches!(filter_fn, LuaVal::Nil) {
                    matched.push(cid);
                } else {
                    let mut fargs = vec![LuaVal::Card(cid)];
                    fargs.extend(extra_args.clone());
                    let mut sub_eng = Some(&mut *eng);
                    if let Ok(res) = vm.call_value(filter_fn.clone(), fargs, &mut sub_eng) {
                        if res.first().map(|v| v.is_truthy()).unwrap_or(false) {
                            matched.push(cid);
                        }
                    }
                }
            }

            match method {
                "GetMatchingGroup" => Ok(vec![LuaVal::Group(vm.alloc_group(matched))]),
                "GetMatchingGroupCount" => Ok(vec![LuaVal::Int(matched.len() as i64)]),
                "IsExistingMatchingCard" | "IsExistingTarget" => Ok(vec![LuaVal::Bool(matched.len() >= count_min)]),
                "GetFirstMatchingCard" => Ok(vec![matched.first().copied().map(LuaVal::Card).unwrap_or(LuaVal::Nil)]),
                "SelectMatchingCard" | "SelectTarget" => {
                    let max = args.get(base_idx + 5).map(|v| v.as_u32() as usize).unwrap_or(count_min.max(1));
                    // Prefer user-provided targets in `current_chain_targets` if they match!
                    let mut selected: Vec<CardId> = eng
                        .state
                        .current_chain_targets
                        .iter()
                        .copied()
                        .filter(|cid| matched.contains(cid))
                        .take(max)
                        .collect();
                    for cid in &matched {
                        if selected.len() >= count_min {
                            break;
                        }
                        if !selected.contains(cid) {
                            selected.push(*cid);
                        }
                    }
                    if method == "SelectTarget" {
                        eng.state.current_chain_targets = selected.clone();
                    }
                    Ok(vec![LuaVal::Group(vm.alloc_group(selected))])
                }
                _ => Ok(vec![LuaVal::Nil]),
            }
        }
        "GetFieldGroup" => {
            let tp = args.first().map(|v| v.as_u32() as PlayerId).unwrap_or(0);
            let s = Location::from_bits_truncate(args.get(1).map(|v| v.as_u32()).unwrap_or(0));
            let o = Location::from_bits_truncate(args.get(2).map(|v| v.as_u32()).unwrap_or(0));
            let mut out = Vec::new();
            if !s.is_empty() { out.extend(eng.cards_in(tp, s)); }
            if !o.is_empty() { out.extend(eng.cards_in(eng.opponent_of(tp), o)); }
            Ok(vec![LuaVal::Group(vm.alloc_group(out))])
        }
        "GetFieldGroupCount" => {
            let tp = args.first().map(|v| v.as_u32() as PlayerId).unwrap_or(0);
            let s = Location::from_bits_truncate(args.get(1).map(|v| v.as_u32()).unwrap_or(0));
            let o = Location::from_bits_truncate(args.get(2).map(|v| v.as_u32()).unwrap_or(0));
            let mut cnt = 0usize;
            if !s.is_empty() { cnt += eng.cards_in(tp, s).len(); }
            if !o.is_empty() { cnt += eng.cards_in(eng.opponent_of(tp), o).len(); }
            Ok(vec![LuaVal::Int(cnt as i64)])
        }
        "GetFirstTarget" => {
            let first = eng.state.current_chain_targets.first().copied();
            Ok(vec![first.map(LuaVal::Card).unwrap_or(LuaVal::Nil)])
        }
        "GetTargetCards" => {
            let g = vm.alloc_group(eng.state.current_chain_targets.clone());
            Ok(vec![LuaVal::Group(g)])
        }
        "SetTargetCard" => {
            eng.state.current_chain_targets = extract_cards(vm, args.first());
            Ok(vec![])
        }
        "GetChainInfo" => {
            let info = args.get(1).map(|v| v.as_u32()).unwrap_or(0);
            if info == 0x08 {
                Ok(vec![LuaVal::Group(vm.alloc_group(eng.state.current_chain_targets.clone()))])
            } else {
                Ok(vec![LuaVal::Int(0)])
            }
        }
        "NegateActivation" => {
            let link = args.first().map(|v| v.as_u32() as u8).unwrap_or(0);
            Ok(vec![LuaVal::Bool(eng.state.chain.negate_activation(link))])
        }
        "NegateEffect" => {
            let link = args.first().map(|v| v.as_u32() as u8).unwrap_or(0);
            Ok(vec![LuaVal::Bool(eng.state.chain.negate_effect(link))])
        }
        "RegisterEffect" => {
            if let Some(LuaVal::Effect(draft_id)) = args.first() {
                if let Some(mut eff) = vm.draft_effects.remove(draft_id) {
                    eff.is_player_effect = true;
                    eff.owner_player = args.get(1).map(|v| v.as_u32() as PlayerId).unwrap_or(0);
                    let id = eng.register_effect(eff, &mut noop);
                    vm.effect_id_aliases.insert(*draft_id, id);
                    return Ok(vec![LuaVal::Int(id as i64)]);
                }
            }
            Ok(vec![LuaVal::Int(0)])
        }
        "GetTurnPlayer" => Ok(vec![LuaVal::Int(eng.state.turn_player as i64)]),
        "GetTurnCount" => Ok(vec![LuaVal::Int(eng.state.turn_count as i64)]),
        "GetCurrentPhase" => Ok(vec![LuaVal::Int(eng.state.phase.code() as i64)]),
        "GetCurrentChain" => Ok(vec![LuaVal::Int(eng.state.chain.len() as i64)]),
        "GetAttacker" => Ok(vec![eng.state.battle.attacker.map(LuaVal::Card).unwrap_or(LuaVal::Nil)]),
        "GetAttackTarget" => Ok(vec![eng.state.battle.defender.map(LuaVal::Card).unwrap_or(LuaVal::Nil)]),
        "NegateAttack" => {
            eng.state.battle.attack_negated = true;
            Ok(vec![LuaVal::Bool(true)])
        }
        "TossCoin" => {
            let p = args.first().map(|v| v.as_u32() as PlayerId).unwrap_or(0);
            let cnt = args.get(1).map(|v| v.as_u32() as usize).unwrap_or(1);
            let res: Vec<u8> = (0..cnt).map(|_| eng.state.rng.toss_coin()).collect();
            eng.emit(DuelMessage::TossCoin { player: p, results: res.clone() });
            Ok(res.into_iter().map(|r| LuaVal::Int(r as i64)).collect())
        }
        "TossDice" => {
            let p = args.first().map(|v| v.as_u32() as PlayerId).unwrap_or(0);
            let cnt = args.get(1).map(|v| v.as_u32() as usize).unwrap_or(1);
            let res: Vec<u8> = (0..cnt).map(|_| eng.state.rng.roll_dice()).collect();
            eng.emit(DuelMessage::TossDice { player: p, results: res.clone() });
            Ok(res.into_iter().map(|r| LuaVal::Int(r as i64)).collect())
        }
        "ShuffleDeck" => {
            let p = args.first().map(|v| v.as_u32() as PlayerId).unwrap_or(0);
            eng.shuffle_deck(p);
            Ok(vec![])
        }
        "ConfirmCards" => {
            let p = args.first().map(|v| v.as_u32() as PlayerId).unwrap_or(0);
            let cards = extract_cards(vm, args.get(1));
            eng.emit(DuelMessage::ConfirmCards { player: p, cards });
            Ok(vec![])
        }
        "Win" => {
            let p = args.first().map(|v| v.as_u32() as PlayerId).unwrap_or(0);
            eng.declare_win(Some(p), WinReason::SpecialWinEffect);
            Ok(vec![])
        }
        "CreateToken" => {
            let p = args.first().map(|v| v.as_u32() as PlayerId).unwrap_or(0);
            let code = args.get(1).map(|v| v.as_u32()).unwrap_or(0);
            let tok = CardData {
                code,
                alias: 0,
                setcodes: vec![],
                card_type: CardType::MONSTER | CardType::TOKEN | CardType::NORMAL,
                atk: 0,
                def: 0,
                level: 1,
                race: Race::FIEND,
                attribute: Attribute::DARK,
                lscale: 0,
                rscale: 0,
                link_markers: LinkMarker::empty(),
                ot: 3,
                category: Category::empty(),
                name: format!("Token #{code}"),
                desc: "Special Summoned Token".into(),
                strings: vec![],
                maximum_atk: None,
                genesys_points: 0,
            };
            let cid = eng.add_card(tok, p, Location::HAND, Position::FACEUP_ATTACK, &mut noop);
            Ok(vec![LuaVal::Card(cid)])
        }
        "GetOperatedGroup" => Ok(vec![LuaVal::Group(vm.alloc_group(vm.operated_group.clone()))]),
        "SetChainLimit" => {
            eng.state.chain.no_further_responses = true;
            Ok(vec![])
        }
        "SetOperationInfo" | "Hint" | "ShuffleHand" | "SetTargetPlayer" | "SetTargetParam" => Ok(vec![]),
        _ => Ok(vec![LuaVal::Nil]),
    }
}

fn call_group_api(
    vm: &mut LuaVm,
    method: &str,
    args: Vec<LuaVal>,
    engine: &mut Option<&mut DuelEngine>,
) -> Result<Vec<LuaVal>, LuaError> {
    match method {
        "CreateGroup" => Ok(vec![LuaVal::Group(vm.alloc_group(Vec::new()))]),
        "FromCards" => {
            let cards: Vec<CardId> = args
                .iter()
                .filter_map(|v| match v {
                    LuaVal::Card(cid) => Some(*cid),
                    _ => None,
                })
                .collect();
            Ok(vec![LuaVal::Group(vm.alloc_group(cards))])
        }
        _ => {
            let Some(LuaVal::Group(gid)) = args.first().cloned() else {
                return Ok(vec![LuaVal::Nil]);
            };
            match method {
                "GetCount" => {
                    let n = vm.groups.get(&gid).map(|g| g.cards.len()).unwrap_or(0);
                    Ok(vec![LuaVal::Int(n as i64)])
                }
                "GetFirst" => {
                    let c = vm.groups.get_mut(&gid).and_then(|g| g.first());
                    Ok(vec![c.map(LuaVal::Card).unwrap_or(LuaVal::Nil)])
                }
                "GetNext" => {
                    let c = vm.groups.get_mut(&gid).and_then(|g| g.next());
                    Ok(vec![c.map(LuaVal::Card).unwrap_or(LuaVal::Nil)])
                }
                "AddCard" => {
                    if let Some(LuaVal::Card(cid)) = args.get(1) {
                        if let Some(g) = vm.groups.get_mut(&gid) {
                            g.add(*cid);
                        }
                    }
                    Ok(vec![])
                }
                "RemoveCard" => {
                    if let Some(LuaVal::Card(cid)) = args.get(1) {
                        if let Some(g) = vm.groups.get_mut(&gid) {
                            g.remove(*cid);
                        }
                    }
                    Ok(vec![])
                }
                "Merge" => {
                    let other = extract_cards(vm, args.get(1));
                    if let Some(g) = vm.groups.get_mut(&gid) {
                        for c in other {
                            g.add(c);
                        }
                    }
                    Ok(vec![])
                }
                "Sub" => {
                    let other = extract_cards(vm, args.get(1));
                    if let Some(g) = vm.groups.get_mut(&gid) {
                        for c in other {
                            g.remove(c);
                        }
                    }
                    Ok(vec![])
                }
                "Clone" => {
                    let cards = vm.groups.get(&gid).map(|g| g.cards.clone()).unwrap_or_default();
                    Ok(vec![LuaVal::Group(vm.alloc_group(cards))])
                }
                "Clear" => {
                    if let Some(g) = vm.groups.get_mut(&gid) {
                        g.cards.clear();
                        g.cursor = 0;
                    }
                    Ok(vec![])
                }
                "IsContains" => {
                    let ok = match args.get(1) {
                        Some(LuaVal::Card(cid)) => vm.groups.get(&gid).map(|g| g.cards.contains(cid)).unwrap_or(false),
                        _ => false,
                    };
                    Ok(vec![LuaVal::Bool(ok)])
                }
                "Filter" | "FilterCount" | "IsExists" => {
                    let cards = vm.groups.get(&gid).map(|g| g.cards.clone()).unwrap_or_default();
                    let f = args.get(1).cloned().unwrap_or(LuaVal::Nil);
                    let (min_cnt, exc_idx) = if method == "IsExists" {
                        (args.get(2).map(|v| v.as_u32() as usize).unwrap_or(1), 3)
                    } else {
                        (0, 2)
                    };
                    let excluded = extract_cards(vm, args.get(exc_idx));
                    let extra = if args.len() > exc_idx + 1 { args[(exc_idx + 1)..].to_vec() } else { Vec::new() };
                    let mut out = Vec::new();
                    for cid in cards {
                        if excluded.contains(&cid) {
                            continue;
                        }
                        if matches!(f, LuaVal::Nil) {
                            out.push(cid);
                        } else {
                            let mut fargs = vec![LuaVal::Card(cid)];
                            fargs.extend(extra.clone());
                            if let Ok(res) = vm.call_value(f.clone(), fargs, engine) {
                                if res.first().map(|v| v.is_truthy()).unwrap_or(false) {
                                    out.push(cid);
                                }
                            }
                        }
                    }
                    match method {
                        "Filter" => Ok(vec![LuaVal::Group(vm.alloc_group(out))]),
                        "FilterCount" => Ok(vec![LuaVal::Int(out.len() as i64)]),
                        "IsExists" => Ok(vec![LuaVal::Bool(out.len() >= min_cnt)]),
                        _ => Ok(vec![LuaVal::Nil]),
                    }
                }
                "GetSum" => {
                    let cards = vm.groups.get(&gid).map(|g| g.cards.clone()).unwrap_or_default();
                    let f = args.get(1).cloned().unwrap_or(LuaVal::Nil);
                    let mut sum = 0i64;
                    for cid in cards {
                        if let Ok(res) = vm.call_value(f.clone(), vec![LuaVal::Card(cid)], engine) {
                            sum += res.first().map(|v| v.as_int()).unwrap_or(0);
                        }
                    }
                    Ok(vec![LuaVal::Int(sum)])
                }
                "Select" => {
                    let min = args.get(2).map(|v| v.as_u32() as usize).unwrap_or(1);
                    let cards: Vec<CardId> = vm.groups.get(&gid).map(|g| g.cards.iter().copied().take(min).collect()).unwrap_or_default();
                    Ok(vec![LuaVal::Group(vm.alloc_group(cards))])
                }
                "ForEach" => {
                    let cards = vm.groups.get(&gid).map(|g| g.cards.clone()).unwrap_or_default();
                    let f = args.get(1).cloned().unwrap_or(LuaVal::Nil);
                    for cid in cards {
                        let _ = vm.call_value(f.clone(), vec![LuaVal::Card(cid)], engine);
                    }
                    Ok(vec![])
                }
                "KeepAlive" | "DeleteGroup" => Ok(vec![]),
                _ => Ok(vec![LuaVal::Nil]),
            }
        }
    }
}

/// Official YGOPro / YGO Omega `Debug.*` Puzzle API!
fn call_debug_api(
    vm: &mut LuaVm,
    method: &str,
    args: Vec<LuaVal>,
    engine: &mut Option<&mut DuelEngine>,
) -> Result<Vec<LuaVal>, LuaError> {
    match method {
        "SetAIName" => {
            vm.puzzle_ai_name = args.first().map(|v| v.as_str()).unwrap_or_default();
            Ok(vec![])
        }
        "ShowHint" | "Message" => {
            let msg = args.first().map(|v| v.as_str()).unwrap_or_default();
            vm.puzzle_hints.push(msg.clone());
            if let Some(eng) = engine.as_deref_mut() {
                eng.emit(DuelMessage::Hint { player: 0, message: msg });
            }
            Ok(vec![])
        }
        "ReloadFieldBegin" => {
            if let Some(eng) = engine.as_deref_mut() {
                eng.state.cards.clear();
                eng.state.effects.clear();
                eng.state.config.flags.insert(DuelFlags::NO_SHUFFLE_DECK);
            }
            Ok(vec![])
        }
        "AddCard" => {
            if let Some(eng) = engine.as_deref_mut() {
                let code = args.first().map(|v| v.as_u32()).unwrap_or(0);
                let owner = args.get(1).map(|v| v.as_u32() as PlayerId).unwrap_or(0);
                let ctrl = args.get(2).map(|v| v.as_u32() as PlayerId).unwrap_or(owner);
                let loc = Location::from_bits_truncate(args.get(3).map(|v| v.as_u32()).unwrap_or(Location::MZONE.bits()));
                let seq = args.get(4).map(|v| v.as_u32() as u8).unwrap_or(0);
                let pos = Position::from_bits_truncate(args.get(5).map(|v| v.as_u32()).unwrap_or(Position::FACEUP_ATTACK.bits()));
                let proc_ok = args.get(6).map(|v| v.is_truthy()).unwrap_or(false);
                let data = vm.card_catalog.get(&code).cloned().unwrap_or_else(|| CardData {
                    code,
                    alias: 0,
                    setcodes: vec![],
                    card_type: CardType::MONSTER | CardType::EFFECT,
                    atk: 2000,
                    def: 1500,
                    level: 4,
                    race: Race::WARRIOR,
                    attribute: Attribute::LIGHT,
                    lscale: 0,
                    rscale: 0,
                    link_markers: LinkMarker::empty(),
                    ot: 3,
                    category: Category::empty(),
                    name: format!("Card #{code}"),
                    desc: String::new(),
                    strings: vec![],
                    maximum_atk: None,
                    genesys_points: 0,
                });
                let mut noop = NoopScriptHost;
                let cid = eng.add_card_at(data, owner, ctrl, loc, seq, pos, proc_ok, &mut noop);
                return Ok(vec![LuaVal::Card(cid)]);
            }
            Ok(vec![LuaVal::Nil])
        }
        "ReloadFieldEnd" => {
            if let Some(eng) = engine.as_deref_mut() {
                let mut noop = NoopScriptHost;
                eng.start_puzzle(0, &mut noop);
            }
            Ok(vec![])
        }
        "PreAddCounter" => {
            if let (Some(eng), Some(LuaVal::Card(cid))) = (engine.as_deref_mut(), args.first()) {
                let ct = args.get(1).map(|v| v.as_u32() as u16).unwrap_or(1);
                let cnt = args.get(2).map(|v| v.as_u32()).unwrap_or(1);
                if let Some(c) = eng.card_mut(*cid) {
                    c.add_counter(ct, cnt);
                }
            }
            Ok(vec![])
        }
        "PreSummon" => {
            if let (Some(eng), Some(LuaVal::Card(cid))) = (engine.as_deref_mut(), args.first()) {
                let stype = SummonType::from_bits_truncate(args.get(1).map(|v| v.as_u32()).unwrap_or(SummonType::SPECIAL.bits()));
                if let Some(c) = eng.card_mut(*cid) {
                    c.summon_type = stype;
                    c.status.insert(CardStatus::PROC_COMPLETE);
                }
            }
            Ok(vec![])
        }
        "PreEquip" => {
            if let (Some(eng), Some(LuaVal::Card(eq)), Some(LuaVal::Card(t))) = (engine.as_deref_mut(), args.first(), args.get(1)) {
                if let Some(ec) = eng.card_mut(*eq) { ec.equip_target = Some(*t); }
                if let Some(tc) = eng.card_mut(*t) { tc.equipped_cards.push(*eq); }
            }
            Ok(vec![])
        }
        _ => Ok(vec![LuaVal::Nil]),
    }
}

fn call_aux_api(_vm: &mut LuaVm, method: &str, args: Vec<LuaVal>) -> Result<Vec<LuaVal>, LuaError> {
    match method {
        "Stringid" => {
            let code = args.first().map(|v| v.as_int()).unwrap_or(0);
            let id = args.get(1).map(|v| v.as_int()).unwrap_or(0);
            Ok(vec![LuaVal::Int((code << 4) + id)])
        }
        _ => Ok(vec![LuaVal::Nil]),
    }
}
