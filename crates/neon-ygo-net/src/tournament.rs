use std::collections::{BTreeMap, BTreeSet};
use serde::{Deserialize, Serialize};
use neon_ygo_core::FormatPreset;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TournamentKind {
    Swiss { rounds: u32 },
    SingleElimination,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pairing {
    pub table: u32,
    pub player_a: String,
    pub player_b: Option<String>, // None = Bye
    pub result: Option<MatchOutcome>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchOutcome {
    PlayerAWins,
    PlayerBWins,
    Draw,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StandingRow {
    pub rank: usize,
    pub player: String,
    pub match_points: u32,
    pub wins: u32,
    pub losses: u32,
    pub draws: u32,
    pub omw_percent: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tournament {
    pub id: String,
    pub name: String,
    pub format: FormatPreset,
    pub kind: TournamentKind,
    pub entrants: Vec<String>,
    pub current_round: u32,
    pub rounds: Vec<Vec<Pairing>>,
    pub finished: bool,
    pub champion: Option<String>,
    played_pairs: BTreeSet<(String, String)>,
    had_bye: BTreeSet<String>,
}

impl Tournament {
    pub fn new(id: impl Into<String>, name: impl Into<String>, format: FormatPreset, kind: TournamentKind) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            format,
            kind,
            entrants: Vec::new(),
            current_round: 0,
            rounds: Vec::new(),
            finished: false,
            champion: None,
            played_pairs: BTreeSet::new(),
            had_bye: BTreeSet::new(),
        }
    }

    pub fn register(&mut self, player: impl Into<String>) -> bool {
        let p = player.into();
        if self.current_round > 0 || self.entrants.contains(&p) {
            return false;
        }
        self.entrants.push(p);
        true
    }

    /// Generate the next round's pairings (Swiss with rematch avoidance, or Single-Elimination bracket).
    pub fn start_next_round(&mut self) -> Result<Vec<Pairing>, String> {
        if self.finished {
            return Err("tournament already finished".into());
        }
        if self.entrants.len() < 2 {
            return Err("need at least 2 players".into());
        }
        if let Some(last) = self.rounds.last() {
            if last.iter().any(|p| p.result.is_none()) {
                return Err("current round still has unreported matches".into());
            }
        }

        let pairings = match self.kind {
            TournamentKind::Swiss { rounds } => {
                if self.current_round >= rounds {
                    self.finalize();
                    return Err("all Swiss rounds completed".into());
                }
                self.pair_swiss()
            }
            TournamentKind::SingleElimination => self.pair_single_elim()?,
        };

        self.current_round += 1;
        self.rounds.push(pairings.clone());
        Ok(pairings)
    }

    fn pair_swiss(&mut self) -> Vec<Pairing> {
        let standings = self.standings();
        let mut pool: Vec<String> = if standings.is_empty() {
            self.entrants.clone()
        } else {
            standings.into_iter().map(|s| s.player).collect()
        };

        let mut pairings = Vec::new();
        let mut table = 1u32;

        // If odd number of players, give Bye to lowest-ranked player who hasn't had a Bye
        if pool.len() % 2 == 1 {
            let bye_idx = pool
                .iter()
                .rposition(|p| !self.had_bye.contains(p))
                .unwrap_or(pool.len() - 1);
            let bye_player = pool.remove(bye_idx);
            self.had_bye.insert(bye_player.clone());
            pairings.push(Pairing {
                table,
                player_a: bye_player,
                player_b: None,
                result: Some(MatchOutcome::PlayerAWins),
            });
            table += 1;
        }

        while pool.len() >= 2 {
            let a = pool.remove(0);
            let opp_idx = pool
                .iter()
                .position(|b| !self.has_played(&a, b))
                .unwrap_or(0);
            let b = pool.remove(opp_idx);
            self.record_pair(&a, &b);
            pairings.push(Pairing {
                table,
                player_a: a,
                player_b: Some(b),
                result: None,
            });
            table += 1;
        }
        pairings
    }

    fn pair_single_elim(&mut self) -> Result<Vec<Pairing>, String> {
        let active: Vec<String> = if let Some(last) = self.rounds.last() {
            last.iter()
                .filter_map(|p| match p.result {
                    Some(MatchOutcome::PlayerAWins) => Some(p.player_a.clone()),
                    Some(MatchOutcome::PlayerBWins) => p.player_b.clone(),
                    Some(MatchOutcome::Draw) => Some(p.player_a.clone()),
                    None => None,
                })
                .collect()
        } else {
            self.entrants.clone()
        };

        if active.len() <= 1 {
            self.champion = active.first().cloned();
            self.finished = true;
            return Err("bracket already decided".into());
        }

        let mut pairings = Vec::new();
        let mut table = 1u32;
        let mut i = 0usize;
        while i < active.len() {
            let a = active[i].clone();
            if i + 1 < active.len() {
                let b = active[i + 1].clone();
                pairings.push(Pairing {
                    table,
                    player_a: a,
                    player_b: Some(b),
                    result: None,
                });
            } else {
                pairings.push(Pairing {
                    table,
                    player_a: a,
                    player_b: None,
                    result: Some(MatchOutcome::PlayerAWins),
                });
            }
            table += 1;
            i += 2;
        }
        Ok(pairings)
    }

    pub fn report_result(&mut self, table: u32, outcome: MatchOutcome) -> Result<(), String> {
        let round = self.rounds.last_mut().ok_or("no active round")?;
        let pairing = round
            .iter_mut()
            .find(|p| p.table == table)
            .ok_or("table not found")?;
        pairing.result = Some(outcome);

        // Auto-finalize if final round finished
        let all_done = round.iter().all(|p| p.result.is_some());
        if all_done {
            match self.kind {
                TournamentKind::Swiss { rounds } if self.current_round >= rounds => self.finalize(),
                TournamentKind::SingleElimination if round.len() == 1 => {
                    self.champion = match outcome {
                        MatchOutcome::PlayerAWins | MatchOutcome::Draw => Some(round[0].player_a.clone()),
                        MatchOutcome::PlayerBWins => round[0].player_b.clone(),
                    };
                    self.finished = true;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn finalize(&mut self) {
        self.finished = true;
        self.champion = self.standings().first().map(|s| s.player.clone());
    }

    /// Compute official Swiss standings sorted by Match Points (3/win, 1/draw) then OMW% tiebreaker!
    pub fn standings(&self) -> Vec<StandingRow> {
        let mut stats: BTreeMap<String, (u32, u32, u32, Vec<String>)> = BTreeMap::new();
        for p in &self.entrants {
            stats.insert(p.clone(), (0, 0, 0, Vec::new()));
        }
        for round in &self.rounds {
            for p in round {
                let Some(res) = p.result else { continue };
                match (&p.player_b, res) {
                    (None, _) => {
                        if let Some(s) = stats.get_mut(&p.player_a) {
                            s.0 += 1;
                        }
                    }
                    (Some(b), MatchOutcome::PlayerAWins) => {
                        if let Some(sa) = stats.get_mut(&p.player_a) { sa.0 += 1; sa.3.push(b.clone()); }
                        if let Some(sb) = stats.get_mut(b) { sb.1 += 1; sb.3.push(p.player_a.clone()); }
                    }
                    (Some(b), MatchOutcome::PlayerBWins) => {
                        if let Some(sa) = stats.get_mut(&p.player_a) { sa.1 += 1; sa.3.push(b.clone()); }
                        if let Some(sb) = stats.get_mut(b) { sb.0 += 1; sb.3.push(p.player_a.clone()); }
                    }
                    (Some(b), MatchOutcome::Draw) => {
                        if let Some(sa) = stats.get_mut(&p.player_a) { sa.2 += 1; sa.3.push(b.clone()); }
                        if let Some(sb) = stats.get_mut(b) { sb.2 += 1; sb.3.push(p.player_a.clone()); }
                    }
                }
            }
        }

        let win_pct: BTreeMap<String, f64> = stats
            .iter()
            .map(|(p, (w, l, d, _))| {
                let g = (*w + *l + *d) as f64;
                let pct = if g > 0.0 { ((*w as f64) * 3.0 + (*d as f64)) / (g * 3.0) } else { 0.33 };
                (p.clone(), pct.max(0.33)) // Konami floor of 33% for OMW%
            })
            .collect();

        let mut rows: Vec<StandingRow> = stats
            .into_iter()
            .map(|(player, (wins, losses, draws, opps))| {
                let omw = if opps.is_empty() {
                    0.33
                } else {
                    opps.iter().map(|o| win_pct.get(o).copied().unwrap_or(0.33)).sum::<f64>()
                        / (opps.len() as f64)
                };
                StandingRow {
                    rank: 0,
                    player,
                    match_points: wins * 3 + draws,
                    wins,
                    losses,
                    draws,
                    omw_percent: omw,
                }
            })
            .collect();

        rows.sort_by(|a, b| {
            b.match_points
                .cmp(&a.match_points)
                .then_with(|| b.omw_percent.partial_cmp(&a.omw_percent).unwrap_or(std::cmp::Ordering::Equal))
                .then_with(|| a.player.cmp(&b.player))
        });
        for (idx, r) in rows.iter_mut().enumerate() {
            r.rank = idx + 1;
        }
        rows
    }

    fn pair_key(a: &str, b: &str) -> (String, String) {
        if a <= b { (a.to_string(), b.to_string()) } else { (b.to_string(), a.to_string()) }
    }

    fn has_played(&self, a: &str, b: &str) -> bool {
        self.played_pairs.contains(&Self::pair_key(a, b))
    }

    fn record_pair(&mut self, a: &str, b: &str) {
        self.played_pairs.insert(Self::pair_key(a, b));
    }
}
