use serde::{Deserialize, Serialize};
use neon_ygo_core::{CardCode, Mt19937};
use neon_ygo_db::{CardDatabase, Deck};

/// Draft & Sealed Cube Session (1:1 YGO Omega Draft Mode).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftSession {
    pub players: Vec<String>,
    pub total_rounds: usize,
    pub pack_size: usize,
    pub current_round: usize,
    /// Active pack currently in front of each player seat.
    pub active_packs: Vec<Vec<CardCode>>,
    /// Drafted card pools per seat.
    pub pools: Vec<Vec<CardCode>>,
    /// Pending picks for the current pass (`None` until player picks).
    pub pending_picks: Vec<Option<usize>>,
    pub finished: bool,
    rng: Mt19937,
    cube: Vec<CardCode>,
}

impl DraftSession {
    pub fn new(
        players: Vec<String>,
        cube: Vec<CardCode>,
        total_rounds: usize,
        pack_size: usize,
        seed: u32,
    ) -> Self {
        let n = players.len();
        let mut session = Self {
            players,
            total_rounds,
            pack_size,
            current_round: 0,
            active_packs: vec![Vec::new(); n],
            pools: vec![Vec::new(); n],
            pending_picks: vec![None; n],
            finished: false,
            rng: Mt19937::new(seed),
            cube,
        };
        session.open_next_packs();
        session
    }

    /// Generate a Sealed pool directly (e.g., 6 packs x 15 cards) for one player.
    pub fn generate_sealed_pool(cube: &[CardCode], packs: usize, pack_size: usize, seed: u32) -> Vec<CardCode> {
        let mut rng = Mt19937::new(seed);
        let mut pool = Vec::with_capacity(packs * pack_size);
        if cube.is_empty() {
            return pool;
        }
        for _ in 0..(packs * pack_size) {
            let idx = rng.next_range(0, (cube.len() - 1) as u32) as usize;
            pool.push(cube[idx]);
        }
        pool
    }

    fn open_next_packs(&mut self) {
        if self.current_round >= self.total_rounds || self.cube.is_empty() {
            self.finished = true;
            return;
        }
        self.current_round += 1;
        for seat in 0..self.players.len() {
            let mut pack = Vec::with_capacity(self.pack_size);
            for _ in 0..self.pack_size {
                let idx = self.rng.next_range(0, (self.cube.len() - 1) as u32) as usize;
                pack.push(self.cube[idx]);
            }
            self.active_packs[seat] = pack;
            self.pending_picks[seat] = None;
        }
    }

    /// Submit a player's pick index from their active pack. Once all seats pick, packs rotate!
    pub fn pick(&mut self, seat: usize, card_index: usize) -> Result<bool, String> {
        if self.finished {
            return Err("draft already finished".into());
        }
        let pack = self.active_packs.get(seat).ok_or("invalid seat")?;
        if card_index >= pack.len() {
            return Err("card index out of bounds".into());
        }
        self.pending_picks[seat] = Some(card_index);

        if self.pending_picks.iter().all(|p| p.is_some()) {
            // Commit picks
            for s in 0..self.players.len() {
                let idx = self.pending_picks[s].take().unwrap();
                let card = self.active_packs[s].remove(idx);
                self.pools[s].push(card);
            }
            if self.active_packs.iter().all(|p| p.is_empty()) {
                self.open_next_packs();
            } else {
                // Rotate packs left on odd rounds, right on even rounds (official draft direction!)
                if self.current_round % 2 == 1 {
                    self.active_packs.rotate_left(1);
                } else {
                    self.active_packs.rotate_right(1);
                }
            }
            return Ok(true);
        }
        Ok(false)
    }

    /// Automatically build a playable `Deck` from a player's drafted pool.
    pub fn build_deck_for_seat(&self, seat: usize, db: &CardDatabase) -> Deck {
        let mut deck = Deck {
            name: format!("{} Draft Deck", self.players.get(seat).cloned().unwrap_or_default()),
            ..Default::default()
        };
        if let Some(pool) = self.pools.get(seat) {
            for &code in pool {
                if db.get(code).map(|c| c.is_extra_deck()).unwrap_or(false) {
                    if deck.extra.len() < 15 {
                        deck.extra.push(code);
                    }
                } else if deck.main.len() < 40 {
                    deck.main.push(code);
                } else if deck.side.len() < 15 {
                    deck.side.push(code);
                }
            }
        }
        deck
    }
}
