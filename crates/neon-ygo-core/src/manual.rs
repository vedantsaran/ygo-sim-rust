use serde::{Deserialize, Serialize};
use crate::card::{CardData, CardId};
use crate::constants::*;

/// Manual / Judge / Sandbox commands matching YGO Omega's Manual Mode & Referee controls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ManualCommand {
    /// Toggle Manual Mode on/off at any time during a duel.
    SetManualMode(bool),
    /// Move any card to any zone/controller/position without rule restrictions.
    MoveCard {
        card_id: CardId,
        to_controller: PlayerId,
        to_location: Location,
        to_sequence: Option<u8>,
        to_position: Position,
    },
    /// Flip or change a card's battle/face position directly.
    SetPosition {
        card_id: CardId,
        position: Position,
    },
    /// Set a player's Life Points to an exact amount.
    SetLp {
        player: PlayerId,
        lp: i32,
    },
    /// Adjust a player's Life Points by a delta (+/-).
    AdjustLp {
        player: PlayerId,
        delta: i32,
    },
    /// Override a monster's ATK (`None` clears manual override).
    SetAtkOverride {
        card_id: CardId,
        atk: Option<i32>,
    },
    /// Override a monster's DEF (`None` clears manual override).
    SetDefOverride {
        card_id: CardId,
        def: Option<i32>,
    },
    /// Add counters to a card.
    AddCounter {
        card_id: CardId,
        counter_type: u16,
        count: u32,
    },
    /// Remove counters from a card.
    RemoveCounter {
        card_id: CardId,
        counter_type: u16,
        count: u32,
    },
    /// Draw `count` cards for a player.
    DrawCards {
        player: PlayerId,
        count: usize,
    },
    /// Shuffle a player's deck.
    ShuffleDeck {
        player: PlayerId,
    },
    /// Spawn a custom Token directly onto the field.
    SpawnToken {
        controller: PlayerId,
        data: CardData,
        position: Position,
        sequence: Option<u8>,
    },
    /// Attach a card as Xyz Material (Overlay Unit) under an Xyz monster.
    AttachOverlay {
        material: CardId,
        xyz_monster: CardId,
    },
    /// Detach an Xyz Material to GY.
    DetachOverlay {
        xyz_monster: CardId,
        material: CardId,
    },
    /// Toss `count` coins.
    TossCoin {
        player: PlayerId,
        count: usize,
    },
    /// Roll `count` 6-sided dice.
    RollDice {
        player: PlayerId,
        count: usize,
    },
    /// Jump directly to a phase.
    SetPhase(Phase),
    /// Advance to the next turn.
    ForceNextTurn,
    /// Judge ruling: declare a duel winner (or `None` for draw).
    DeclareWinner {
        winner: Option<PlayerId>,
        reason: WinReason,
    },
    /// Undo/rollback the duel to the most recent state snapshot!
    Undo,
}
