use std::collections::BTreeMap;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use neon_ygo_core::*;
use neon_ygo_db::{Banlist, CardDatabase, Deck};

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MatchError {
    #[error("side deck violation: {0}")]
    InvalidSideDeck(String),
    #[error("invalid match state: {0}")]
    InvalidState(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchPhase {
    WaitingRps,
    WaitingTurnOrder { chooser: PlayerId },
    InGame { game_number: u8 },
    SideDecking { next_game: u8, loser_chooses: PlayerId },
    Finished { winner: Option<PlayerId> },
}

/// Complete Best-of-3 (or Single) Match Coordinator (1:1 YGO Omega!).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchSession {
    pub is_best_of_three: bool,
    pub phase: MatchPhase,
    pub score: [u8; 2],
    pub original_decks: [Deck; 2],
    pub current_decks: [Deck; 2],
    pub rps_choices: [Option<RpsMove>; 2],
    pub side_ready: [bool; 2],
    pub game_first_player: PlayerId,
}

impl MatchSession {
    pub fn new(is_best_of_three: bool, deck0: Deck, deck1: Deck) -> Self {
        Self {
            is_best_of_three,
            phase: MatchPhase::WaitingRps,
            score: [0, 0],
            original_decks: [deck0.clone(), deck1.clone()],
            current_decks: [deck0, deck1],
            rps_choices: [None, None],
            side_ready: [false, false],
            game_first_player: 0,
        }
    }

    /// Submit Rock-Paper-Scissors choice. Ties automatically reset for a rematch!
    pub fn submit_rps(&mut self, player: PlayerId, mv: RpsMove) -> Result<MatchPhase, MatchError> {
        if self.phase != MatchPhase::WaitingRps {
            return Err(MatchError::InvalidState("not in RPS phase".into()));
        }
        self.rps_choices[(player % 2) as usize] = Some(mv);
        if let (Some(a), Some(b)) = (self.rps_choices[0], self.rps_choices[1]) {
            match RpsMove::winner(a, b) {
                Some(winner) => {
                    self.phase = MatchPhase::WaitingTurnOrder { chooser: winner };
                }
                None => {
                    self.rps_choices = [None, None];
                }
            }
        }
        Ok(self.phase)
    }

    /// RPS winner (Game 1) or previous game loser (Game 2/3) chooses to Go First or Go Second.
    pub fn choose_turn_order(&mut self, player: PlayerId, go_first: bool) -> Result<MatchPhase, MatchError> {
        let MatchPhase::WaitingTurnOrder { chooser } = self.phase else {
            return Err(MatchError::InvalidState("not waiting for turn order".into()));
        };
        if player != chooser {
            return Err(MatchError::InvalidState("only the designated chooser can pick turn order".into()));
        }
        self.game_first_player = if go_first { chooser } else { 1 - chooser };
        let game_num = self.score[0] + self.score[1] + 1;
        self.phase = MatchPhase::InGame { game_number: game_num };
        Ok(self.phase)
    }

    /// Report the winner of the current game; advances to Side Decking or Match Completion.
    pub fn finish_game(&mut self, game_winner: Option<PlayerId>) -> Result<MatchPhase, MatchError> {
        let MatchPhase::InGame { game_number } = self.phase else {
            return Err(MatchError::InvalidState("not currently in a game".into()));
        };
        if let Some(w) = game_winner {
            self.score[(w % 2) as usize] += 1;
        }
        let needed = if self.is_best_of_three { 2 } else { 1 };
        if self.score[0] >= needed {
            self.phase = MatchPhase::Finished { winner: Some(0) };
        } else if self.score[1] >= needed {
            self.phase = MatchPhase::Finished { winner: Some(1) };
        } else if !self.is_best_of_three {
            self.phase = MatchPhase::Finished { winner: game_winner };
        } else {
            let loser = match game_winner {
                Some(0) => 1,
                Some(1) => 0,
                _ => 1 - self.game_first_player,
            };
            self.side_ready = [false, false];
            self.phase = MatchPhase::SideDecking {
                next_game: game_number + 1,
                loser_chooses: loser,
            };
        }
        Ok(self.phase)
    }

    /// Validate & submit a Side-Decked list between games!
    /// Official Konami Tournament Policy:
    /// - Exact 1:1 multiset conservation of `(Main + Extra + Side)` cards against the player's registered deck!
    /// - Validated against format & banlist!
    pub fn submit_side_deck(
        &mut self,
        player: PlayerId,
        new_deck: Deck,
        db: &CardDatabase,
        banlist: &Banlist,
        config: &DuelConfig,
    ) -> Result<MatchPhase, MatchError> {
        let MatchPhase::SideDecking { loser_chooses, .. } = self.phase else {
            return Err(MatchError::InvalidState("not in Side Decking phase".into()));
        };
        let idx = (player % 2) as usize;
        if card_multiset(&self.original_decks[idx]) != card_multiset(&new_deck) {
            return Err(MatchError::InvalidSideDeck(
                "Side-decked card pool does not match registered Main+Extra+Side multiset!".into(),
            ));
        }
        let report = banlist.validate_deck(&new_deck, db, config);
        if !report.valid {
            return Err(MatchError::InvalidSideDeck(report.errors.join("; ")));
        }
        self.current_decks[idx] = new_deck;
        self.side_ready[idx] = true;
        if self.side_ready[0] && self.side_ready[1] {
            self.phase = MatchPhase::WaitingTurnOrder { chooser: loser_chooses };
        }
        Ok(self.phase)
    }
}

fn card_multiset(deck: &Deck) -> BTreeMap<CardCode, u32> {
    let mut map = BTreeMap::new();
    for &c in deck.main.iter().chain(deck.extra.iter()).chain(deck.side.iter()) {
        *map.entry(c).or_insert(0) += 1;
    }
    map
}

/// Mask private hidden information for an opponent or spectator!
/// Prevents cheating: opponents/spectators cannot see hidden hands, face-down deck order,
/// face-down set cards, or face-down banished cards (unless `open_hands == true` for spectator mode).
pub fn mask_state_for_viewer(
    state: &DuelState,
    viewer: Option<PlayerId>,
    spectator_open_hands: bool,
) -> DuelState {
    let mut masked = state.clone();
    for card in masked.cards.values_mut() {
        let is_owner = viewer.map(|v| (v % 2) == (card.controller % 2)).unwrap_or(false);
        let hide = match card.location {
            Location::DECK => true,
            Location::EXTRA => card.is_facedown() && !is_owner,
            Location::HAND => !is_owner && !(viewer.is_none() && spectator_open_hands),
            Location::MZONE | Location::SZONE | Location::FZONE | Location::PZONE => {
                card.is_facedown() && !is_owner
            }
            Location::REMOVED => card.is_facedown() && !is_owner,
            _ => false,
        };
        if hide {
            card.data.code = 0;
            card.data.name = "Face-Down Card".into();
            card.data.desc.clear();
            card.data.atk = 0;
            card.data.def = 0;
            card.data.level = 0;
            card.computed.code = 0;
            card.computed.atk = 0;
            card.computed.def = 0;
        }
    }
    masked
}
