use neon_ygo_core::*;
use neon_ygo_db::{CardDatabase, Deck};
use neon_ygo_lua::LuaScriptHost;

/// Solo Hand Test & Combo Sandbox (1:1 YGO Omega Test Hand / Solo Mode).
pub struct HandTestSandbox {
    pub engine: DuelEngine,
    pub host: LuaScriptHost,
    pub deck: Deck,
    pub seed: u32,
}

impl HandTestSandbox {
    pub fn new(deck: Deck, format: FormatPreset, seed: u32, db: &CardDatabase) -> Self {
        let mut cfg = DuelConfig::from_preset(format, seed);
        cfg.flags.insert(DuelFlags::MANUAL_MODE);
        let mut engine = DuelEngine::new(cfg, "Solo Tester", "Practice Dummy");
        let mut host = LuaScriptHost::new();
        db.attach_to_script_host(&mut host);

        let (main, extra, skill) = deck.resolve(db);
        engine.load_deck(0, main.clone(), extra.clone(), skill.clone(), &mut host);
        engine.load_deck(1, main, extra, skill, &mut host);
        engine.start_duel(0, &mut host);

        Self {
            engine,
            host,
            deck,
            seed,
        }
    }

    /// Re-shuffle with a new seed and draw a fresh opening hand!
    pub fn redraw_opening_hand(&mut self, new_seed: u32, db: &CardDatabase) {
        *self = Self::new(self.deck.clone(), self.engine.state.config.format, new_seed, db);
    }

    /// Draw 1 extra test card.
    pub fn draw_one(&mut self) -> Vec<CardId> {
        self.engine.draw_cards(0, 1, Reason::RULE, &mut self.host)
    }

    /// Current opening hand card names.
    pub fn hand_names(&self) -> Vec<String> {
        self.engine
            .cards_in(0, Location::HAND)
            .into_iter()
            .filter_map(|id| self.engine.card(id))
            .map(|c| c.data.name.clone())
            .collect()
    }
}
