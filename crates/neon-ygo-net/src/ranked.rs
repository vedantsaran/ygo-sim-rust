use std::collections::BTreeMap;
use serde::{Deserialize, Serialize};
use neon_ygo_core::FormatPreset;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RankTier {
    Bronze,
    Silver,
    Gold,
    Platinum,
    Diamond,
    Omega,
}

impl RankTier {
    pub fn from_elo(elo: i32) -> Self {
        match elo {
            ..=1199 => RankTier::Bronze,
            1200..=1399 => RankTier::Silver,
            1400..=1599 => RankTier::Gold,
            1600..=1799 => RankTier::Platinum,
            1800..=1999 => RankTier::Diamond,
            _ => RankTier::Omega,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerRating {
    pub player: String,
    pub elo: i32,
    pub wins: u32,
    pub losses: u32,
    pub draws: u32,
    pub streak: i32,
}

impl PlayerRating {
    pub fn new(player: impl Into<String>) -> Self {
        Self {
            player: player.into(),
            elo: 1200,
            wins: 0,
            losses: 0,
            draws: 0,
            streak: 0,
        }
    }

    pub fn games_played(&self) -> u32 {
        self.wins + self.losses + self.draws
    }

    pub fn tier(&self) -> RankTier {
        RankTier::from_elo(self.elo)
    }

    fn k_factor(&self) -> f64 {
        if self.games_played() < 10 {
            32.0
        } else if self.elo >= 2000 {
            16.0
        } else {
            24.0
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueTicket {
    pub player: String,
    pub format: FormatPreset,
    pub elo: i32,
    pub max_spread: i32,
}

/// Multi-format Seasonal Ranked Ladder & Matchmaking Queue (1:1 YGO Omega).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RankedLadder {
    pub season: u32,
    pub ratings: BTreeMap<(FormatPreset, String), PlayerRating>,
    pub queue: Vec<QueueTicket>,
}

impl RankedLadder {
    pub fn new(season: u32) -> Self {
        Self {
            season,
            ratings: BTreeMap::new(),
            queue: Vec::new(),
        }
    }

    pub fn get_or_create(&mut self, format: FormatPreset, player: &str) -> &mut PlayerRating {
        self.ratings
            .entry((format, player.to_string()))
            .or_insert_with(|| PlayerRating::new(player))
    }

    pub fn rating_of(&self, format: FormatPreset, player: &str) -> i32 {
        self.ratings
            .get(&(format, player.to_string()))
            .map(|r| r.elo)
            .unwrap_or(1200)
    }

    /// Record a ranked result (`score_a`: `1.0` = A won, `0.0` = B won, `0.5` = draw)
    /// and update both players' Elo ratings. Returns `(delta_a, delta_b)`.
    pub fn record_match(
        &mut self,
        format: FormatPreset,
        player_a: &str,
        player_b: &str,
        score_a: f64,
    ) -> (i32, i32) {
        let ra = self.get_or_create(format, player_a).clone();
        let rb = self.get_or_create(format, player_b).clone();

        let exp_a = 1.0 / (1.0 + 10f64.powf((rb.elo - ra.elo) as f64 / 400.0));
        let exp_b = 1.0 - exp_a;
        let score_b = 1.0 - score_a;

        let da = (ra.k_factor() * (score_a - exp_a)).round() as i32;
        let db = (rb.k_factor() * (score_b - exp_b)).round() as i32;

        {
            let a = self.get_or_create(format, player_a);
            a.elo = (a.elo + da).max(100);
            if score_a > 0.75 {
                a.wins += 1;
                a.streak = if a.streak > 0 { a.streak + 1 } else { 1 };
            } else if score_a < 0.25 {
                a.losses += 1;
                a.streak = if a.streak < 0 { a.streak - 1 } else { -1 };
            } else {
                a.draws += 1;
                a.streak = 0;
            }
        }
        {
            let b = self.get_or_create(format, player_b);
            b.elo = (b.elo + db).max(100);
            if score_b > 0.75 {
                b.wins += 1;
                b.streak = if b.streak > 0 { b.streak + 1 } else { 1 };
            } else if score_b < 0.25 {
                b.losses += 1;
                b.streak = if b.streak < 0 { b.streak - 1 } else { -1 };
            } else {
                b.draws += 1;
                b.streak = 0;
            }
        }
        (da, db)
    }

    pub fn leaderboard(&self, format: FormatPreset, top_n: usize) -> Vec<PlayerRating> {
        let mut list: Vec<PlayerRating> = self
            .ratings
            .iter()
            .filter(|((f, _), _)| *f == format)
            .map(|(_, r)| r.clone())
            .collect();
        list.sort_by(|a, b| b.elo.cmp(&a.elo).then_with(|| b.wins.cmp(&a.wins)));
        list.truncate(top_n);
        list
    }

    /// Enqueue a player for Ranked Matchmaking; returns a matched `(player_a, player_b)` pair if found!
    pub fn enqueue(&mut self, format: FormatPreset, player: impl Into<String>, max_spread: i32) -> Option<(String, String)> {
        let name = player.into();
        self.queue.retain(|t| t.player != name);
        let elo = self.rating_of(format, &name);
        if let Some(pos) = self.queue.iter().position(|t| {
            t.format == format && (t.elo - elo).abs() <= max_spread.min(t.max_spread)
        }) {
            let opp = self.queue.remove(pos);
            return Some((opp.player, name));
        }
        self.queue.push(QueueTicket {
            player: name,
            format,
            elo,
            max_spread,
        });
        None
    }
}
