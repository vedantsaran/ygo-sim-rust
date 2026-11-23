use serde::{Deserialize, Serialize};
use crate::card::CardId;
use crate::constants::*;
use crate::effect::EffectId;

/// A single link on the active Yu-Gi-Oh! Chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainLink {
    pub link_number: u8,
    pub effect_id: EffectId,
    pub card_id: CardId,
    pub triggering_player: PlayerId,
    pub triggering_location: Location,
    pub triggering_sequence: u8,
    pub triggering_position: Position,
    pub spell_speed: u8,
    pub targets: Vec<CardId>,
    pub activation_negated: bool,
    pub effect_negated: bool,
    pub category: Category,
    pub label: i64,
    /// Requires the activating card (Continuous Spell/Trap, Field Spell, Equip Spell, Pendulum Scale)
    /// to remain face-up on the field at resolution!
    pub requires_faceup_on_field_to_resolve: bool,
}

/// Candidate trigger collected for SEGOC (Simultaneous Effects Go On Chain).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingTrigger {
    pub effect_id: EffectId,
    pub card_id: CardId,
    pub controller: PlayerId,
    pub mandatory: bool,
    pub spell_speed: u8,
    pub description: u32,
}

/// Official Konami SEGOC priority bucket:
/// 1. Turn Player's Mandatory Triggers
/// 2. Opponent's Mandatory Triggers
/// 3. Turn Player's Optional Triggers
/// 4. Opponent's Optional Triggers
pub fn sort_segoc_triggers(triggers: &mut [PendingTrigger], turn_player: PlayerId) {
    triggers.sort_by_key(|t| {
        let is_turn_player = (t.controller % 2) == (turn_player % 2);
        let bucket: u8 = match (t.mandatory, is_turn_player) {
            (true, true) => 0,
            (true, false) => 1,
            (false, true) => 2,
            (false, false) => 3,
        };
        (bucket, t.effect_id)
    });
}

/// Active Chain stack state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainStack {
    pub links: Vec<ChainLink>,
    pub resolving_index: Option<usize>,
    /// Optional chain limit preventing responses (e.g., Super Polymerization, Accesscode Talker, Counter Trap).
    pub chain_limit_min_spell_speed: u8,
    pub no_further_responses: bool,
    pub no_opponent_responses_for: Option<PlayerId>,
}

impl ChainStack {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.links.len()
    }

    pub fn is_empty(&self) -> bool {
        self.links.is_empty()
    }

    pub fn current_spell_speed(&self) -> u8 {
        self.links.last().map(|l| l.spell_speed).unwrap_or(0)
    }

    /// Official Konami rule:
    /// - Chain Link 1 can be Spell Speed 1, 2, or 3.
    /// - Responses to an existing Chain must be >= Spell Speed 2 AND >= previous link's Spell Speed.
    /// - Specifically, only Spell Speed 3 (Counter Trap) can chain to Spell Speed 3!
    pub fn can_chain(&self, candidate_spell_speed: u8, candidate_player: PlayerId) -> bool {
        if self.no_further_responses {
            return false;
        }
        if let Some(blocked_opp_of) = self.no_opponent_responses_for {
            if (candidate_player % 2) != (blocked_opp_of % 2) {
                return false;
            }
        }
        if self.links.is_empty() {
            return candidate_spell_speed >= 1;
        }
        let required = self.current_spell_speed().max(2).max(self.chain_limit_min_spell_speed);
        candidate_spell_speed >= required
    }

    pub fn push(&mut self, mut link: ChainLink) -> u8 {
        let num = (self.links.len() + 1) as u8;
        link.link_number = num;
        self.links.push(link);
        num
    }

    pub fn get(&self, link_number: u8) -> Option<&ChainLink> {
        if link_number == 0 {
            return None;
        }
        self.links.get((link_number - 1) as usize)
    }

    pub fn get_mut(&mut self, link_number: u8) -> Option<&mut ChainLink> {
        if link_number == 0 {
            return None;
        }
        self.links.get_mut((link_number - 1) as usize)
    }

    pub fn negate_activation(&mut self, link_number: u8) -> bool {
        if let Some(link) = self.get_mut(link_number) {
            link.activation_negated = true;
            return true;
        }
        false
    }

    pub fn negate_effect(&mut self, link_number: u8) -> bool {
        if let Some(link) = self.get_mut(link_number) {
            link.effect_negated = true;
            return true;
        }
        false
    }

    pub fn clear(&mut self) {
        self.links.clear();
        self.resolving_index = None;
        self.chain_limit_min_spell_speed = 0;
        self.no_further_responses = false;
        self.no_opponent_responses_for = None;
    }
}
