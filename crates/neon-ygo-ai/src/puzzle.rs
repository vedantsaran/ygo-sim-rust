use serde::{Deserialize, Serialize};
use neon_ygo_core::*;
use neon_ygo_db::CardDatabase;
use neon_ygo_lua::{LuaError, LuaScriptHost};

/// A YGOPro / YGO Omega Duel Puzzle (supports standard `.lua` `Debug.*` puzzle scripts!).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuelPuzzle {
    pub id: String,
    pub title: String,
    pub difficulty: String,
    pub objective: String,
    pub lua_script: String,
}

pub struct PuzzleSession {
    pub puzzle: DuelPuzzle,
    pub engine: DuelEngine,
    pub host: LuaScriptHost,
}

impl PuzzleSession {
    /// Load and initialize any `.lua` puzzle script using `Debug.ReloadFieldBegin` / `Debug.AddCard` / `Debug.ReloadFieldEnd`!
    pub fn start(puzzle: DuelPuzzle, db: &CardDatabase) -> Result<Self, LuaError> {
        let mut cfg = DuelConfig::from_preset(FormatPreset::TcgAdvanced, 1);
        cfg.flags.insert(DuelFlags::NO_SHUFFLE_DECK);
        let mut engine = DuelEngine::new(cfg, "Puzzle Solver", "Puzzle AI");
        let mut host = LuaScriptHost::new();
        db.attach_to_script_host(&mut host);

        host.run_puzzle_script(&mut engine, &puzzle.lua_script)?;
        // Allow Turn 1 Battle Phase in Puzzles (official YGOPro puzzle convention: DUEL_ATTACK_FIRST_TURN!)
        engine.state.turn_count = 2;
        // Initialize scripts on every card placed by Debug.AddCard!
        let all_ids: Vec<CardId> = engine.state.cards.keys().copied().collect();
        for cid in all_ids {
            host.init_card(&mut engine, cid);
        }
        engine.recalculate_effects(&mut host);

        Ok(Self {
            puzzle,
            engine,
            host,
        })
    }

    pub fn is_solved(&self) -> bool {
        self.engine.state.winner == Some(Some(0))
    }

    pub fn hints(&self) -> &[String] {
        &self.host.vm.puzzle_hints
    }
}

/// Built-in playable YGO Omega Duel Puzzles written in authentic `Debug.*` Lua!
pub fn builtin_puzzles() -> Vec<DuelPuzzle> {
    vec![
        DuelPuzzle {
            id: "otk-raigeki-blue-eyes".into(),
            title: "Blue-Eyes Wipe & OTK".into(),
            difficulty: "Beginner".into(),
            objective: "Win this turn using Raigeki, Monster Reborn, and Blue-Eyes White Dragon!".into(),
            lua_script: r#"
Debug.SetAIName("Kaiba Bot")
Debug.ReloadFieldBegin(DUEL_ATTACK_FIRST_TURN|DUEL_SIMPLE_AI,5)
Duel.SetLP(0,1000)
Duel.SetLP(1,3000)
-- Player 0 Hand: Raigeki & Monster Reborn
Debug.AddCard(12580477,0,0,LOCATION_HAND,0,POS_FACEUP_ATTACK)
Debug.AddCard(83764718,0,0,LOCATION_HAND,1,POS_FACEUP_ATTACK)
-- Player 0 GY: Blue-Eyes White Dragon (3000 ATK)
Debug.AddCard(89631139,0,0,LOCATION_GRAVE,0,POS_FACEUP_ATTACK)
-- Opponent Field: Dark Magician (2500 ATK)
Debug.AddCard(46986414,1,1,LOCATION_MZONE,2,POS_FACEUP_ATTACK)
Debug.ReloadFieldEnd()
Debug.ShowHint("Clear Dark Magician with Raigeki, revive Blue-Eyes with Monster Reborn, and attack directly for 3000!")
"#.into(),
        },
        DuelPuzzle {
            id: "synchro-climb-stardust".into(),
            title: "Synchro Climb to Stardust".into(),
            difficulty: "Intermediate".into(),
            objective: "Synchro Summon Stardust Dragon (Level 8) and win in Battle Phase!".into(),
            lua_script: r#"
Debug.SetAIName("Synchro Master")
Debug.ReloadFieldBegin(DUEL_ATTACK_FIRST_TURN|DUEL_SIMPLE_AI,5)
Duel.SetLP(0,2000)
Duel.SetLP(1,2500)
-- Player 0 Field: Junk Synchron (Lv3 Tuner) + Cyber Dragon (Lv5 non-Tuner) -> 3 + 5 = 8!
Debug.AddCard(63977008,0,0,LOCATION_MZONE,0,POS_FACEUP_ATTACK)
Debug.AddCard(70095154,0,0,LOCATION_MZONE,1,POS_FACEUP_ATTACK)
-- Player 0 Extra Deck: Stardust Dragon (Lv8, 2500 ATK)
Debug.AddCard(44508094,0,0,LOCATION_EXTRA,0,POS_FACEDOWN_DEFENSE)
Debug.ReloadFieldEnd()
Debug.ShowHint("Tune Level 3 Junk Synchron + Level 5 Cyber Dragon into Level 8 Stardust Dragon, then attack directly!")
"#.into(),
        },
        DuelPuzzle {
            id: "exodia-pot-of-greed".into(),
            title: "Heart of the Cards: Exodia Draw".into(),
            difficulty: "Beginner".into(),
            objective: "Draw the final piece of Exodia with Pot of Greed to win immediately!".into(),
            lua_script: r#"
Debug.SetAIName("Rare Hunter")
Debug.ReloadFieldBegin(DUEL_ATTACK_FIRST_TURN|DUEL_SIMPLE_AI,5)
Duel.SetLP(0,500)
Duel.SetLP(1,8000)
Debug.AddCard(33396948,0,0,LOCATION_HAND,0,POS_FACEUP_ATTACK)
Debug.AddCard(7902349,0,0,LOCATION_HAND,1,POS_FACEUP_ATTACK)
Debug.AddCard(70903634,0,0,LOCATION_HAND,2,POS_FACEUP_ATTACK)
Debug.AddCard(8124921,0,0,LOCATION_HAND,3,POS_FACEUP_ATTACK)
Debug.AddCard(55144522,0,0,LOCATION_HAND,4,POS_FACEUP_ATTACK)
Debug.AddCard(89631139,0,0,LOCATION_DECK,0,POS_FACEDOWN_DEFENSE)
Debug.AddCard(44519536,0,0,LOCATION_DECK,1,POS_FACEDOWN_DEFENSE)
Debug.ReloadFieldEnd()
Debug.ShowHint("Activate Pot of Greed to assemble all 5 pieces of Exodia the Forbidden One!")
"#.into(),
        },
    ]
}
