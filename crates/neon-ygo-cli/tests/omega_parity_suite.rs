use std::process::Command;

use neon_ygo_ai::{builtin_puzzles, AiDifficulty, DuelAi, HandTestSandbox, PuzzleSession};
use neon_ygo_core::*;
use neon_ygo_db::{
    hypergeometric_at_least, load_cdb_file, Banlist, CardDatabase, DeckFilter, SortBy,
};
use neon_ygo_lua::{LuaScriptHost, LuaVal, LuaVm};
use neon_ygo_net::{
    ctos, decode_frame, encode_frame, mask_state_for_viewer, ClientPacket, DraftSession,
    MatchOutcome, MatchPhase, MatchSession, OmegaClient, OmegaServer, RankedLadder, RankTier,
    RoomMode, ServerPacket, Tournament, TournamentKind,
};
use neon_ygo_replay::{Replay, ReplayPlayer, YRPX_MAGIC, YRP_MAGIC};

// ============================================================================
// 1. MASTER RULES 1-5 & FORMAT RULESETS
// ============================================================================

#[test]
fn test_master_rules_1_through_5() {
    let db = CardDatabase::with_builtin();
    let stardust = db.get(44508094).unwrap();
    let decode = db.get(1861629).unwrap();

    // MR1 / MR2: Turn 1 Draw enabled, Shared Field Spell, no EMZ
    let mr1 = DuelConfig::from_preset(FormatPreset::TcgAdvanced, 1).with_master_rule(MasterRule::Mr1);
    assert!(mr1.flags.contains(DuelFlags::TURN1_DRAW));
    assert!(mr1.flags.contains(DuelFlags::SHARED_FIELD_SPELL));
    assert!(!mr1.has_emz());

    // MR3: Separate Pendulum Zones (6 & 7), Turn 1 skip draw, two Field Spells
    let mr3 = DuelConfig::from_preset(FormatPreset::TcgAdvanced, 1).with_master_rule(MasterRule::Mr3);
    assert!(!mr3.flags.contains(DuelFlags::TURN1_DRAW));
    assert!(!mr3.flags.contains(DuelFlags::SHARED_FIELD_SPELL));
    assert_eq!(mr3.pendulum_zone_seqs(), (6, 7));

    // MR4: ALL Extra Deck monsters (including Synchro) require EMZ or Linked MMZ
    let mr4 = DuelConfig::from_preset(FormatPreset::TcgAdvanced, 1).with_master_rule(MasterRule::Mr4);
    let eng_mr4 = DuelEngine::new(mr4, "A", "B");
    assert_eq!(eng_mr4.valid_extra_deck_zones(0, stardust), vec![5, 6]);

    // MR5 (2020 Revision): Fusion/Synchro/Xyz free to MMZ; Link & face-up Pendulum require EMZ/Link
    let mr5 = DuelConfig::from_preset(FormatPreset::TcgAdvanced, 1).with_master_rule(MasterRule::Mr5);
    let eng_mr5 = DuelEngine::new(mr5, "A", "B");
    assert_eq!(eng_mr5.valid_extra_deck_zones(0, stardust), vec![0, 1, 2, 3, 4, 5, 6]);
    assert_eq!(eng_mr5.valid_extra_deck_zones(0, decode), vec![5, 6]);
}

#[test]
fn test_goat_shared_field_spell_and_turn1_draw() {
    let db = CardDatabase::with_builtin();
    let cfg = DuelConfig::from_preset(FormatPreset::Goat, 10);
    assert!(cfg.flags.contains(DuelFlags::OBSOLETE_IGNITION));
    assert!(cfg.flags.contains(DuelFlags::TURN1_DRAW));
    assert!(cfg.flags.contains(DuelFlags::SHARED_FIELD_SPELL));

    let mut eng = DuelEngine::new(cfg, "Goat1", "Goat2");
    let mut host = LuaScriptHost::new();
    db.attach_to_script_host(&mut host);
    for _ in 0..10 {
        eng.add_card(db.get(89631139).unwrap().clone(), 0, Location::DECK, Position::FACEDOWN_DEFENSE, &mut host);
        eng.add_card(db.get(46986414).unwrap().clone(), 1, Location::DECK, Position::FACEDOWN_DEFENSE, &mut host);
    }
    eng.start_duel(0, &mut host);
    // In GOAT (MR1), Turn 1 player draws -> starts Turn 1 with 6 cards in hand!
    assert_eq!(eng.cards_in(0, Location::HAND).len(), 6);

    // Shared Field Spell rule: P1 activating Field Spell sends P0's Field Spell to GY!
    let f0 = eng.add_card_at(db.get(22702055).unwrap().clone(), 0, 0, Location::FZONE, 5, Position::FACEUP_ATTACK, false, &mut host);
    let f1 = eng.add_card(db.get(47355498).unwrap().clone(), 1, Location::HAND, Position::FACEUP_ATTACK, &mut host);
    eng.state.turn_player = 1;
    eng.set_spell_trap(1, f1, None, &mut host).unwrap();
    assert_eq!(eng.card(f0).unwrap().location, Location::GRAVE);
    assert_eq!(eng.card(f1).unwrap().location, Location::FZONE);
}

#[test]
fn test_rush_duel_draw_to_5_unlimited_normal_summons_and_maximum() {
    let db = CardDatabase::with_builtin();
    let mut eng = DuelEngine::new(DuelConfig::from_preset(FormatPreset::RushDuel, 42), "Yuga", "Luke");
    let mut host = LuaScriptHost::new();
    db.attach_to_script_host(&mut host);

    for _ in 0..15 {
        eng.add_card(db.get(26202165).unwrap().clone(), 0, Location::DECK, Position::FACEDOWN_DEFENSE, &mut host);
        eng.add_card(db.get(26202165).unwrap().clone(), 1, Location::DECK, Position::FACEDOWN_DEFENSE, &mut host);
    }
    eng.start_duel(0, &mut host);
    // Opening hand 4 -> Rush Draw Phase draws until 5!
    assert_eq!(eng.cards_in(0, Location::HAND).len(), 5);

    // Unlimited Normal Summons per turn in Rush Duel! (Summon 3 monsters in 1 turn across 3 MMZ)
    let hand = eng.cards_in(0, Location::HAND);
    eng.normal_summon(0, hand[0], &[], None, false, &mut host).unwrap();
    eng.normal_summon(0, hand[1], &[], None, false, &mut host).unwrap();
    eng.normal_summon(0, hand[2], &[], None, false, &mut host).unwrap();
    assert_eq!(eng.cards_in(0, Location::MZONE).len(), 3);

    // Maximum Summon: sends existing 3 monsters to GY and summons Yggdrago [L][C][R] with 4000 Maximum ATK!
    let l = eng.add_card(db.get(160003001).unwrap().clone(), 0, Location::HAND, Position::FACEUP_ATTACK, &mut host);
    let c = eng.add_card(db.get(160003002).unwrap().clone(), 0, Location::HAND, Position::FACEUP_ATTACK, &mut host);
    let r = eng.add_card(db.get(160003003).unwrap().clone(), 0, Location::HAND, Position::FACEUP_ATTACK, &mut host);
    eng.maximum_summon(0, [l, c, r], &mut host).unwrap();
    assert_eq!(eng.card(c).unwrap().current_atk(), 4000);
    assert!(eng.card(c).unwrap().status.contains(CardStatus::MAXIMUM_MODE));
    assert_eq!(eng.cards_in(0, Location::GRAVE).len(), 3);
}

#[test]
fn test_speed_duel_skill_and_turbo_speed_counters() {
    let db = CardDatabase::with_builtin();
    let mut host = LuaScriptHost::new();
    db.attach_to_script_host(&mut host);

    // Speed Duel: 4000 LP, 3 MMZ, Skill Card activation
    let mut spd = DuelEngine::new(DuelConfig::from_preset(FormatPreset::SpeedDuel, 1), "Yugi", "Kaiba");
    assert_eq!(spd.get_lp(0), 4000);
    assert_eq!(spd.state.config.mmz_count(), 3);
    spd.start_puzzle(0, &mut host);
    spd.add_card(db.get(46986414).unwrap().clone(), 0, Location::DECK, Position::FACEDOWN_DEFENSE, &mut host);
    let sk = spd.add_card(db.get(300101001).unwrap().clone(), 0, Location::SKILL, Position::FACEDOWN_DEFENSE, &mut host);
    spd.activate_skill(0, sk, &mut host).unwrap();
    assert!(spd.state.players[0].skill_used);
    assert_eq!(spd.cards_in(0, Location::HAND).len(), 1);

    // Turbo Duel: +1 Speed Counter every Standby Phase, -1 per 1000 damage taken
    let mut turbo = DuelEngine::new(DuelConfig::from_preset(FormatPreset::TurboDuel, 1), "Yusei", "Jack");
    for _ in 0..10 {
        turbo.add_card(db.get(89631139).unwrap().clone(), 0, Location::DECK, Position::FACEDOWN_DEFENSE, &mut host);
        turbo.add_card(db.get(46986414).unwrap().clone(), 1, Location::DECK, Position::FACEDOWN_DEFENSE, &mut host);
    }
    turbo.start_duel(0, &mut host);
    assert_eq!(turbo.state.players[0].speed_counters, 1);
    turbo.end_turn(&mut host);
    assert_eq!(turbo.state.players[0].speed_counters, 2);
    turbo.inflict_damage(0, 2000, false, Reason::EFFECT, &mut host);
    assert_eq!(turbo.state.players[0].speed_counters, 0);
}

#[test]
fn test_tag_2v2_and_relay_duel() {
    let db = CardDatabase::with_builtin();
    let mut host = NoopScriptHost;

    // Tag Duel: 4 players, 16000 LP, rotation 0 -> 1 -> 2 -> 3 -> 0
    let mut tag = DuelEngine::new(DuelConfig::from_preset(FormatPreset::TagDuel, 1), "P0", "P1");
    tag.set_tag_partners("P2", "P3");
    assert_eq!(tag.get_lp(0), 16000);
    assert_eq!(tag.get_lp(2), 16000);
    for p in 0..4 {
        for _ in 0..10 {
            tag.add_card(db.get(89631139).unwrap().clone(), p, Location::DECK, Position::FACEDOWN_DEFENSE, &mut host);
        }
    }
    tag.start_duel(0, &mut host);
    assert_eq!(tag.state.turn_player, 0);
    tag.end_turn(&mut host);
    assert_eq!(tag.state.turn_player, 1);
    tag.end_turn(&mut host);
    assert_eq!(tag.state.turn_player, 2);
    tag.end_turn(&mut host);
    assert_eq!(tag.state.turn_player, 3);
    tag.end_turn(&mut host);
    assert_eq!(tag.state.turn_player, 0);

    // Relay Duel: Reserve teammate takes over with 8000 LP while KEEPING active field!
    let mut relay = DuelEngine::new(DuelConfig::from_preset(FormatPreset::RelayDuel, 1), "Yugi", "Pegasus");
    relay.add_relay_teammate(0, "Joey", vec![db.get(74677422).unwrap().clone(); 10], vec![]);
    relay.start_puzzle(0, &mut host);
    let dm = relay.add_card_at(db.get(46986414).unwrap().clone(), 0, 0, Location::MZONE, 2, Position::FACEUP_ATTACK, true, &mut host);
    relay.inflict_damage(0, 8000, false, Reason::EFFECT, &mut host);
    assert!(!relay.is_finished());
    assert_eq!(relay.state.players[0].name, "Joey");
    assert_eq!(relay.get_lp(0), 8000);
    assert_eq!(relay.card(dm).unwrap().location, Location::MZONE);
}

// ============================================================================
// 2. DEEP SUMMONING, LINK GEOMETRY, COLUMN & EXTRA LINK RULINGS
// ============================================================================

#[test]
fn test_link_rating_exact_subset_sum_and_summon() {
    assert!(can_match_link_rating(&[2, 2], 4));         // 2 + 2 = 4
    assert!(can_match_link_rating(&[2, 1, 1], 4));      // 2 + 1 + 1 = 4
    assert!(can_match_link_rating(&[3, 2], 4));         // 3 + 1 = 4 (Link-2 counts as 1!)
    assert!(can_match_link_rating(&[2, 2, 1, 1], 4));   // 1 + 1 + 1 + 1 = 4
    assert!(!can_match_link_rating(&[3, 3], 5));        // possible: 2, 4, 6 -> NOT 5!
    assert!(can_match_link_rating(&[2, 2], 3));         // 2 + 1 = 3 IS legal!
    assert!(!can_match_link_rating(&[2, 2], 5));        // max 4 -> NOT 5!
    assert!(!can_match_link_rating(&[1, 1], 3));        // max 2 -> NOT 3!

    let db = CardDatabase::with_builtin();
    let mut eng = DuelEngine::new(DuelConfig::from_preset(FormatPreset::TcgAdvanced, 1), "A", "B");
    let mut host = NoopScriptHost;
    eng.start_puzzle(0, &mut host);
    let l2 = eng.add_card_at(db.get(2857636).unwrap().clone(), 0, 0, Location::MZONE, 5, Position::FACEUP_ATTACK, true, &mut host); // Link-2
    let m1 = eng.add_card_at(db.get(71413901).unwrap().clone(), 0, 0, Location::MZONE, 0, Position::FACEUP_ATTACK, true, &mut host);
    let m2 = eng.add_card_at(db.get(40044918).unwrap().clone(), 0, 0, Location::MZONE, 1, Position::FACEUP_ATTACK, true, &mut host);
    let accesscode = eng.add_card(db.get(86066372).unwrap().clone(), 0, Location::EXTRA, Position::FACEDOWN_DEFENSE, &mut host);
    eng.link_summon(0, accesscode, &[l2, m1, m2], Some(5), &mut host).unwrap();
    assert_eq!(eng.card(accesscode).unwrap().location, Location::MZONE);
    assert!(eng.card(accesscode).unwrap().is_properly_summoned());
}

#[test]
fn test_column_mechanics_across_players() {
    let db = CardDatabase::with_builtin();
    let mut eng = DuelEngine::new(DuelConfig::from_preset(FormatPreset::TcgAdvanced, 1), "A", "B");
    let mut host = NoopScriptHost;
    // P0 MMZ[1] (col 1), P0 EMZ Left seq 5 (col 1), P1 MMZ[3] (col 4-3 = 1), P0 STZ[1] (col 1) -> ALL SAME COLUMN!
    let p0_m1 = eng.add_card_at(db.get(89631139).unwrap().clone(), 0, 0, Location::MZONE, 1, Position::FACEUP_ATTACK, true, &mut host);
    let emz_l = eng.add_card_at(db.get(1861629).unwrap().clone(), 0, 0, Location::MZONE, 5, Position::FACEUP_ATTACK, true, &mut host);
    let p1_m3 = eng.add_card_at(db.get(46986414).unwrap().clone(), 1, 1, Location::MZONE, 3, Position::FACEUP_ATTACK, true, &mut host);
    let p0_s1 = eng.add_card_at(db.get(10045474).unwrap().clone(), 0, 0, Location::SZONE, 1, Position::FACEUP_ATTACK, false, &mut host);
    let off_col = eng.add_card_at(db.get(74677422).unwrap().clone(), 0, 0, Location::MZONE, 4, Position::FACEUP_ATTACK, true, &mut host);

    let col = eng.column_cards(p0_s1);
    assert!(col.contains(&p0_m1));
    assert!(col.contains(&emz_l));
    assert!(col.contains(&p1_m3));
    assert!(!col.contains(&off_col));
}

#[test]
fn test_xyz_token_forbidden_and_overlay_does_not_trigger_leave_field() {
    let db = CardDatabase::with_builtin();
    let mut eng = DuelEngine::new(DuelConfig::from_preset(FormatPreset::TcgAdvanced, 1), "A", "B");
    let mut host = NoopScriptHost;
    eng.start_puzzle(0, &mut host);

    // Token CANNOT be used as Xyz Material!
    let mut tok_data = db.get(71413901).unwrap().clone();
    tok_data.card_type.insert(CardType::TOKEN);
    let tok = eng.add_card_at(tok_data, 0, 0, Location::MZONE, 0, Position::FACEUP_ATTACK, true, &mut host);
    let m2 = eng.add_card_at(db.get(40044918).unwrap().clone(), 0, 0, Location::MZONE, 1, Position::FACEUP_ATTACK, true, &mut host);
    let uto = eng.add_card(db.get(84013237).unwrap().clone(), 0, Location::EXTRA, Position::FACEDOWN_DEFENSE, &mut host);
    assert!(eng.xyz_summon(0, uto, &[tok, m2], None, &mut host).is_err());

    // Valid Xyz Summon -> Detaching Overlay does NOT raise EVENT_LEAVE_FIELD!
    let m1 = eng.add_card_at(db.get(71413901).unwrap().clone(), 0, 0, Location::MZONE, 2, Position::FACEUP_ATTACK, true, &mut host);
    eng.xyz_summon(0, uto, &[m1, m2], None, &mut host).unwrap();
    eng.state.pending_events.clear();
    let detached = eng.detach_overlay(uto, 1, Reason::COST, 0, &mut host).unwrap();
    assert_eq!(detached.len(), 1);
    assert!(!eng.state.pending_events.iter().any(|e| e.code == event_code::LEAVE_FIELD));
    assert!(eng.state.pending_events.iter().any(|e| e.code == event_code::TO_GRAVE));
}

#[test]
fn test_tokens_vanish_and_cannot_flip_facedown() {
    let db = CardDatabase::with_builtin();
    let mut eng = DuelEngine::new(DuelConfig::from_preset(FormatPreset::TcgAdvanced, 1), "A", "B");
    let mut host = LuaScriptHost::new();
    db.attach_to_script_host(&mut host);
    eng.start_puzzle(0, &mut host);

    let mut tok_data = db.get(71413901).unwrap().clone();
    tok_data.card_type.insert(CardType::TOKEN);
    let tok = eng.add_card_at(tok_data, 1, 1, Location::MZONE, 0, Position::FACEUP_ATTACK, true, &mut host);
    let bom = eng.add_card(db.get(14087893).unwrap().clone(), 0, Location::HAND, Position::FACEUP_ATTACK, &mut host);
    let bom_eff = eng.card(bom).unwrap().effects[0];
    eng.activate_effect(0, bom, bom_eff, vec![tok], &mut host).unwrap();
    eng.resolve_chain(&mut host);
    // Token MUST remain face-up Attack!
    assert_eq!(eng.card(tok).unwrap().position, Position::FACEUP_ATTACK);

    // Destroying Token removes it from the duel completely (NOT in GY!)
    eng.destroy_cards(&[tok], Reason::EFFECT, 0, None, &mut host);
    assert!(eng.card(tok).is_none());
    assert!(eng.cards_in(1, Location::GRAVE).is_empty());
}

#[test]
fn test_gemini_and_spirit_monster_mechanics() {
    let db = CardDatabase::with_builtin();
    let mut eng = DuelEngine::new(DuelConfig::from_preset(FormatPreset::TcgAdvanced, 1), "A", "B");
    let mut host = NoopScriptHost;
    for _ in 0..10 {
        eng.add_card(db.get(89631139).unwrap().clone(), 0, Location::DECK, Position::FACEDOWN_DEFENSE, &mut host);
        eng.add_card(db.get(46986414).unwrap().clone(), 1, Location::DECK, Position::FACEDOWN_DEFENSE, &mut host);
    }
    eng.start_duel(0, &mut host);

    // Gemini: Neos Alius on field starts without Gemini effect until second Normal Summon!
    let alius = eng.add_card_at(db.get(69884162).unwrap().clone(), 0, 0, Location::MZONE, 0, Position::FACEUP_ATTACK, true, &mut host);
    assert!(!eng.card(alius).unwrap().is_gemini_effect_active());
    eng.normal_summon(0, alius, &[], None, false, &mut host).unwrap();
    assert!(eng.card(alius).unwrap().is_gemini_effect_active());

    // Spirit: Tsukuyomi Normal Summoned returns to hand automatically in End Phase!
    eng.state.players[0].normal_summons_used = 0;
    let tsuku = eng.add_card(db.get(34853266).unwrap().clone(), 0, Location::HAND, Position::FACEUP_ATTACK, &mut host);
    eng.normal_summon(0, tsuku, &[], None, false, &mut host).unwrap();
    assert_eq!(eng.card(tsuku).unwrap().location, Location::MZONE);
    eng.end_turn(&mut host);
    assert_eq!(eng.card(tsuku).unwrap().location, Location::HAND);
}

// ============================================================================
// 3. CHAIN, SEGOC, MST vs CONTINUOUS/NORMAL, SKILL DRAIN, OATH vs HARD OPT
// ============================================================================

#[test]
fn test_segoc_four_bucket_priority_order() {
    let mut trigs = vec![
        PendingTrigger { effect_id: 4, card_id: CardId(4), controller: 1, mandatory: false, spell_speed: 1, description: 0 },
        PendingTrigger { effect_id: 3, card_id: CardId(3), controller: 0, mandatory: false, spell_speed: 1, description: 0 },
        PendingTrigger { effect_id: 2, card_id: CardId(2), controller: 1, mandatory: true,  spell_speed: 1, description: 0 },
        PendingTrigger { effect_id: 1, card_id: CardId(1), controller: 0, mandatory: true,  spell_speed: 1, description: 0 },
    ];
    sort_segoc_triggers(&mut trigs, 0);
    let order: Vec<u32> = trigs.iter().map(|t| t.effect_id).collect();
    assert_eq!(order, vec![1, 2, 3, 4]);
}

#[test]
fn test_mst_negates_continuous_spell_but_not_normal_spell() {
    let db = CardDatabase::with_builtin();
    let mut eng = DuelEngine::new(DuelConfig::from_preset(FormatPreset::TcgAdvanced, 1), "A", "B");
    let mut host = LuaScriptHost::new();
    db.attach_to_script_host(&mut host);
    eng.start_puzzle(0, &mut host);
    for _ in 0..5 {
        eng.add_card(db.get(89631139).unwrap().clone(), 0, Location::DECK, Position::FACEDOWN_DEFENSE, &mut host);
    }

    // Case A: MST chained to Normal Spell (Pot of Greed) -> Pot STILL draws 2!
    let pot = eng.add_card(db.get(55144522).unwrap().clone(), 0, Location::HAND, Position::FACEUP_ATTACK, &mut host);
    let mst1 = eng.add_card_at(db.get(5318639).unwrap().clone(), 1, 1, Location::SZONE, 0, Position::FACEDOWN_ATTACK, false, &mut host);
    let pot_e = eng.card(pot).unwrap().effects[0];
    let mst1_e = eng.card(mst1).unwrap().effects[0];
    eng.activate_effect(0, pot, pot_e, vec![], &mut host).unwrap();
    eng.activate_effect(1, mst1, mst1_e, vec![pot], &mut host).unwrap();
    eng.resolve_chain(&mut host);
    assert_eq!(eng.cards_in(0, Location::HAND).len(), 2);

    // Case B: MST chained to Continuous Trap activation (Skill Drain) -> Skill Drain leaves field, resolves WITHOUT effect!
    let sd = eng.add_card_at(db.get(82732705).unwrap().clone(), 0, 0, Location::SZONE, 0, Position::FACEDOWN_ATTACK, false, &mut host);
    let mst2 = eng.add_card_at(db.get(5318639).unwrap().clone(), 1, 1, Location::SZONE, 1, Position::FACEDOWN_ATTACK, false, &mut host);
    let sd_e = eng.card(sd).unwrap().effects[0];
    let mst2_e = eng.card(mst2).unwrap().effects[0];
    eng.activate_effect(0, sd, sd_e, vec![], &mut host).unwrap();
    eng.activate_effect(1, mst2, mst2_e, vec![sd], &mut host).unwrap();
    eng.resolve_chain(&mut host);
    assert_eq!(eng.card(sd).unwrap().location, Location::GRAVE);
    assert!(eng.messages.iter().any(|m| matches!(m, DuelMessage::ChainDisabled { chain_link: 1 })));
}

#[test]
fn test_skill_drain_negates_on_field_but_not_off_field_resolution() {
    let db = CardDatabase::with_builtin();
    let mut eng = DuelEngine::new(DuelConfig::from_preset(FormatPreset::TcgAdvanced, 1), "A", "B");
    let mut host = LuaScriptHost::new();
    db.attach_to_script_host(&mut host);
    eng.start_puzzle(0, &mut host);
    for _ in 0..5 {
        eng.add_card(db.get(89631139).unwrap().clone(), 0, Location::DECK, Position::FACEDOWN_DEFENSE, &mut host);
    }

    // Skill Drain face-up on field
    let _sd = eng.add_card_at(db.get(82732705).unwrap().clone(), 1, 1, Location::SZONE, 0, Position::FACEUP_ATTACK, false, &mut host);
    let mon = eng.add_card_at(db.get(71413901).unwrap().clone(), 0, 0, Location::MZONE, 0, Position::FACEUP_ATTACK, true, &mut host);
    let mut e = eng.create_effect_struct(Some(mon), 0);
    e.effect_type = EffectType::IGNITION;
    e.range = Location::MZONE;
    e.operation = EffectCallback::Builtin("draw_1".into());
    let eid = eng.register_effect(e, &mut host);

    // Activate while face-up on field under Skill Drain -> negated at resolution!
    eng.activate_effect(0, mon, eid, vec![], &mut host).unwrap();
    eng.resolve_chain(&mut host);
    assert_eq!(eng.cards_in(0, Location::HAND).len(), 0);

    // If monster leaves the field before resolution (e.g., Book of Moon or tributed) -> resolves normally!
    eng.activate_effect(0, mon, eid, vec![], &mut host).unwrap();
    eng.move_card(mon, 0, Location::GRAVE, None, Position::FACEUP_ATTACK, Reason::COST, 0, None, &mut host).unwrap();
    eng.resolve_chain(&mut host);
    assert_eq!(eng.cards_in(0, Location::HAND).len(), 1);
}

#[test]
fn test_oath_opt_refunded_on_negated_activation_vs_hard_opt_not_refunded() {
    let db = CardDatabase::with_builtin();
    let mut eng = DuelEngine::new(DuelConfig::from_preset(FormatPreset::TcgAdvanced, 1), "A", "B");
    let mut host = LuaScriptHost::new();
    db.attach_to_script_host(&mut host);
    eng.start_puzzle(0, &mut host);
    for _ in 0..25 {
        eng.add_card(db.get(89631139).unwrap().clone(), 0, Location::DECK, Position::FACEDOWN_DEFENSE, &mut host);
    }

    // Pot of Desires uses EFFECT_COUNT_CODE_OATH ("You can only ACTIVATE 1 Pot of Desires per turn")
    let des1 = eng.add_card(db.get(35261759).unwrap().clone(), 0, Location::HAND, Position::FACEUP_ATTACK, &mut host);
    let des2 = eng.add_card(db.get(35261759).unwrap().clone(), 0, Location::HAND, Position::FACEUP_ATTACK, &mut host);
    let sj = eng.add_card_at(db.get(41420027).unwrap().clone(), 1, 1, Location::SZONE, 0, Position::FACEDOWN_ATTACK, false, &mut host);
    let e1 = eng.card(des1).unwrap().effects[0];
    let e2 = eng.card(des2).unwrap().effects[0];
    let sj_e = eng.card(sj).unwrap().effects[0];

    // Activate Desires #1 -> Solemn Judgment negates ACTIVATION!
    eng.activate_effect(0, des1, e1, vec![], &mut host).unwrap();
    eng.activate_effect(1, sj, sj_e, vec![], &mut host).unwrap();
    eng.resolve_chain(&mut host);
    // Because Desires #1's activation was negated (OATH), player CAN activate Desires #2!
    assert!(eng.can_activate_effect(0, des2, e2, &mut host));
    // And notice Desires #1 banished 10 cards FACE-DOWN as cost!
    let banished = eng.cards_in(0, Location::REMOVED);
    assert_eq!(banished.len(), 10);
    assert!(eng.card(banished[0]).unwrap().is_facedown_banished());
    assert!(!eng.card(banished[0]).unwrap().has_code(89631139));
}

#[test]
fn test_equip_spell_destroyed_when_target_flipped_facedown() {
    let db = CardDatabase::with_builtin();
    let mut eng = DuelEngine::new(DuelConfig::from_preset(FormatPreset::TcgAdvanced, 1), "A", "B");
    let mut host = LuaScriptHost::new();
    db.attach_to_script_host(&mut host);
    eng.start_puzzle(0, &mut host);

    let bewd = eng.add_card_at(db.get(89631139).unwrap().clone(), 0, 0, Location::MZONE, 0, Position::FACEUP_ATTACK, true, &mut host);
    let eq = eng.add_card_at(db.get(56747793).unwrap().clone(), 0, 0, Location::SZONE, 0, Position::FACEUP_ATTACK, false, &mut host);
    eng.card_mut(eq).unwrap().equip_target = Some(bewd);
    eng.card_mut(bewd).unwrap().equipped_cards.push(eq);

    // Book of Moon flips Blue-Eyes face-down -> United We Stand is automatically destroyed!
    let bom = eng.add_card(db.get(14087893).unwrap().clone(), 0, Location::HAND, Position::FACEUP_ATTACK, &mut host);
    let bom_e = eng.card(bom).unwrap().effects[0];
    eng.activate_effect(0, bom, bom_e, vec![bewd], &mut host).unwrap();
    eng.resolve_chain(&mut host);
    assert_eq!(eng.card(bewd).unwrap().position, Position::FACEDOWN_DEFENSE);
    assert_eq!(eng.card(eq).unwrap().location, Location::GRAVE);
}

#[test]
fn test_battle_edge_cases_0_vs_0_and_equal_atk_and_def_recoil() {
    let db = CardDatabase::with_builtin();
    let mut eng = DuelEngine::new(DuelConfig::from_preset(FormatPreset::TcgAdvanced, 1), "A", "B");
    let mut host = NoopScriptHost;
    eng.start_puzzle(0, &mut host);
    eng.state.turn_count = 2;

    // 0 ATK vs 0 ATK -> NEITHER destroyed!
    let a0 = eng.add_card_at(db.get(8240199).unwrap().clone(), 0, 0, Location::MZONE, 0, Position::FACEUP_ATTACK, true, &mut host);
    let b0 = eng.add_card_at(db.get(8240199).unwrap().clone(), 1, 1, Location::MZONE, 0, Position::FACEUP_ATTACK, true, &mut host);
    eng.advance_phase(Phase::BattleStart, &mut host);
    eng.declare_attack(a0, Some(b0), &mut host).unwrap();
    assert_eq!(eng.card(a0).unwrap().location, Location::MZONE);
    assert_eq!(eng.card(b0).unwrap().location, Location::MZONE);

    // Equal ATK > 0 (3000 vs 3000) -> BOTH destroyed!
    let a1 = eng.add_card_at(db.get(89631139).unwrap().clone(), 0, 0, Location::MZONE, 1, Position::FACEUP_ATTACK, true, &mut host);
    let b1 = eng.add_card_at(db.get(89631139).unwrap().clone(), 1, 1, Location::MZONE, 1, Position::FACEUP_ATTACK, true, &mut host);
    eng.declare_attack(a1, Some(b1), &mut host).unwrap();
    assert_eq!(eng.card(a1).unwrap().location, Location::GRAVE);
    assert_eq!(eng.card(b1).unwrap().location, Location::GRAVE);

    // 1000 ATK attacking 2500 DEF -> Defender survives, Attacker takes 1500 recoil!
    let a2 = eng.add_card_at(db.get(26202165).unwrap().clone(), 0, 0, Location::MZONE, 2, Position::FACEUP_ATTACK, true, &mut host);
    let b2 = eng.add_card_at(db.get(89631139).unwrap().clone(), 1, 1, Location::MZONE, 2, Position::FACEUP_DEFENSE, true, &mut host);
    eng.declare_attack(a2, Some(b2), &mut host).unwrap();
    assert_eq!(eng.get_lp(0), 6500);
    assert_eq!(eng.card(a2).unwrap().location, Location::MZONE);
    assert_eq!(eng.card(b2).unwrap().location, Location::MZONE);
}

// ============================================================================
// 4. PURE-RUST SQLITE3 `.cdb` BINARY COMPATIBILITY TEST (REAL SQLITE3 FILE!)
// ============================================================================

#[test]
fn test_pure_rust_sqlite_cdb_reader_against_real_sqlite3_database() {
    let tmp_path = std::env::temp_dir().join("neon_ygo_real_test.cdb");
    let _ = std::fs::remove_file(&tmp_path);

    // Generate a REAL SQLite3 .cdb database using macOS `/usr/bin/sqlite3` with exact YGOPro/Omega schema!
    let sql = r#"
CREATE TABLE datas(id integer primary key,ot integer,alias integer,setcode integer,type integer,atk integer,def integer,level integer,race integer,attribute integer,category integer);
CREATE TABLE texts(id integer primary key,name text,desc text,str1 text,str2 text,str3 text,str4 text,str5 text,str6 text,str7 text,str8 text,str9 text,str10 text,str11 text,str12 text,str13 text,str14 text,str15 text,str16 text);
INSERT INTO datas VALUES(89631139,3,0,221,17,3000,2500,8,8192,16,0);
INSERT INTO texts VALUES(89631139,'Blue-Eyes White Dragon','This legendary dragon is a powerful engine of destruction.','','','','','','','','','','','','','','','','');
INSERT INTO datas VALUES(16178681,3,0,153,16777249,2500,2000,67371015,8192,32,0);
INSERT INTO texts VALUES(16178681,'Odd-Eyes Pendulum Dragon','Scale 4 Pendulum Dragon','','','','','','','','','','','','','','','','');
INSERT INTO datas VALUES(1861629,3,0,257,67108897,2300,133,3,16777216,32,0);
INSERT INTO texts VALUES(1861629,'Decode Talker','Link-3 Cyberse Monster','Boost ATK','','','','','','','','','','','','','','','');
"#;
    let status = Command::new("sqlite3")
        .arg(&tmp_path)
        .arg(sql)
        .status()
        .expect("sqlite3 binary available on macOS");
    assert!(status.success());

    // Now parse that REAL SQLite3 binary file with our 100% PURE-RUST B-tree reader!
    let cards = load_cdb_file(&tmp_path).expect("pure-Rust SQLite3 .cdb parse succeeded");
    let _ = std::fs::remove_file(&tmp_path);

    assert_eq!(cards.len(), 3);
    let bewd = cards.iter().find(|c| c.code == 89631139).unwrap();
    assert_eq!(bewd.name, "Blue-Eyes White Dragon");
    assert_eq!(bewd.atk, 3000);
    assert_eq!(bewd.def, 2500);
    assert_eq!(bewd.level, 8);
    assert!(bewd.is_set_card(0xdd));

    // Verify Pendulum scales unpacked from `(4<<24)|(4<<16)|7 = 67371015`!
    let odd = cards.iter().find(|c| c.code == 16178681).unwrap();
    assert_eq!(odd.level, 7);
    assert_eq!(odd.lscale, 4);
    assert_eq!(odd.rscale, 4);

    // Verify Link Markers unpacked from `def = 133 (0x85 = TOP|BOTTOM_LEFT|BOTTOM_RIGHT)`!
    let decode = cards.iter().find(|c| c.code == 1861629).unwrap();
    assert!(decode.is_link());
    assert_eq!(decode.level, 3);
    assert!(decode.link_markers.contains(LinkMarker::TOP | LinkMarker::BOTTOM_LEFT | LinkMarker::BOTTOM_RIGHT));
    assert_eq!(decode.strings[0], "Boost ATK");
}

// ============================================================================
// 5. PURE-RUST LUA 5.4 VM, .LFLIST.CONF, DECK BUILDER, PUZZLES, AI, NETWORKING
// ============================================================================

#[test]
fn test_pure_rust_lua_vm_language_features() {
    let mut vm = LuaVm::new();
    let res = vm.exec(r#"
local function fact(n)
    if n <= 1 then return 1 end
    return n * fact(n - 1)
end
local t = {10, 20, 30}
table.insert(t, 40)
local sum = 0
for i, v in ipairs(t) do
    sum = sum + v
end
local bits = (0xF0 & 0x3C) | (1 << 8)
return fact(6), sum, bits, #t
"#, None).unwrap();
    assert_eq!(res[0], LuaVal::Int(720));
    assert_eq!(res[1], LuaVal::Int(100));
    assert_eq!(res[2], LuaVal::Int(0x30 | 256));
    assert_eq!(res[3], LuaVal::Int(4));
}

#[test]
fn test_lflist_conf_parser_and_deck_builder_filters() {
    let conf = r#"
# YGO Omega Sample Banlist
!2026.11 TCG
55144522 0 --Pot of Greed
83764718 1 --Monster Reborn
35261759 2 --Pot of Desires

!2005.04 GOAT
55144522 1
12580477 0
"#;
    let parsed = Banlist::parse_lflist_conf(conf);
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].max_copies(55144522), 0);
    assert_eq!(parsed[0].max_copies(83764718), 1);
    assert_eq!(parsed[1].format, FormatPreset::Goat);
    assert_eq!(parsed[1].max_copies(55144522), 1);

    let db = CardDatabase::with_builtin();
    let filter = DeckFilter {
        attribute: Some(Attribute::LIGHT),
        race: Some(Race::DRAGON),
        atk_min: Some(3000),
        sort: SortBy::AtkDesc,
        ..Default::default()
    };
    let hits = filter.search(&db, None);
    assert!(hits.len() >= 3);
    assert_eq!(hits[0].name, "Blue-Eyes Ultimate Dragon");
}

#[test]
fn test_all_three_builtin_lua_puzzles_solvable() {
    let db = CardDatabase::with_builtin();
    let puzzles = builtin_puzzles();

    // Puzzle 1: Blue-Eyes Wipe & OTK (Raigeki + Monster Reborn + Direct Attack for 3000!)
    {
        let mut s = PuzzleSession::start(puzzles[0].clone(), &db).unwrap();
        let hand = s.engine.cards_in(0, Location::HAND);
        let raigeki = hand.iter().copied().find(|&id| s.engine.card(id).unwrap().data.code == 12580477).unwrap();
        let reborn = hand.iter().copied().find(|&id| s.engine.card(id).unwrap().data.code == 83764718).unwrap();
        let bewd = s.engine.cards_in(0, Location::GRAVE)[0];
        let r_e = s.engine.card(raigeki).unwrap().effects[0];
        let m_e = s.engine.card(reborn).unwrap().effects[0];
        s.engine.apply_decision(Decision::ActivateEffect { card_id: raigeki, effect_id: r_e, targets: vec![] }, &mut s.host).unwrap();
        s.engine.apply_decision(Decision::ActivateEffect { card_id: reborn, effect_id: m_e, targets: vec![bewd] }, &mut s.host).unwrap();
        s.engine.apply_decision(Decision::EnterBattlePhase, &mut s.host).unwrap();
        s.engine.apply_decision(Decision::DeclareAttack { attacker: bewd, target: None }, &mut s.host).unwrap();
        assert!(s.is_solved());
    }

    // Puzzle 2: Synchro Climb to Stardust (Junk Synchron Lv3 + Cyber Dragon Lv5 -> Stardust 2500 -> Attack!)
    {
        let mut s = PuzzleSession::start(puzzles[1].clone(), &db).unwrap();
        let mats = s.engine.cards_in(0, Location::MZONE);
        let stardust = s.engine.cards_in(0, Location::EXTRA)[0];
        s.engine.apply_decision(Decision::SpecialSummon { card_id: stardust, materials: mats, zone: None, position: None }, &mut s.host).unwrap();
        s.engine.apply_decision(Decision::EnterBattlePhase, &mut s.host).unwrap();
        s.engine.apply_decision(Decision::DeclareAttack { attacker: stardust, target: None }, &mut s.host).unwrap();
        assert!(s.is_solved());
    }

    // Puzzle 3: Exodia Pot of Greed
    {
        let mut s = PuzzleSession::start(puzzles[2].clone(), &db).unwrap();
        let pot = s.engine.cards_in(0, Location::HAND).into_iter().find(|&id| s.engine.card(id).unwrap().data.code == 55144522).unwrap();
        let pe = s.engine.card(pot).unwrap().effects[0];
        s.engine.apply_decision(Decision::ActivateEffect { card_id: pot, effect_id: pe, targets: vec![] }, &mut s.host).unwrap();
        assert!(s.is_solved());
    }
}

#[test]
fn test_ai_vs_ai_full_duel_and_deterministic_replay_formats() {
    let db = CardDatabase::with_builtin();
    let decks = db.starter_decks();
    let cfg = DuelConfig::from_preset(FormatPreset::TcgAdvanced, 20261123);
    let mut eng = DuelEngine::new(cfg.clone(), "AI-0", "AI-1");
    let mut host = LuaScriptHost::new();
    db.attach_to_script_host(&mut host);

    let (m0, e0, s0) = decks[0].resolve(&db);
    let (m1, e1, s1) = decks[1].resolve(&db);
    eng.load_deck(0, m0, e0, s0, &mut host);
    eng.load_deck(1, m1, e1, s1, &mut host);
    eng.start_duel(0, &mut host);

    let ai0 = DuelAi::new("AI-0", AiDifficulty::Competitive);
    let ai1 = DuelAi::new("AI-1", AiDifficulty::Competitive);
    let mut rep = Replay::new("AI Duel", cfg, 0, vec![("AI-0".into(), decks[0].clone()), ("AI-1".into(), decks[1].clone())]);

    let mut steps = 0;
    while !eng.is_finished() && steps < 250 {
        let ai = if eng.state.turn_player == 0 { &ai0 } else { &ai1 };
        if let Some(d) = ai.choose_decision(&mut eng, &mut host) {
            if eng.apply_decision(d.clone(), &mut host).is_ok() {
                rep.record_decision(d);
            } else {
                rep.record_decision(Decision::EndTurn);
                let _ = eng.apply_decision(Decision::EndTurn, &mut host);
            }
        }
        steps += 1;
    }
    assert!(eng.is_finished(), "AI vs AI duel should finish cleanly within 250 steps");

    // Verify .yrp (LZMA), .yrpx, and .omegarp roundtrips produce bit-identical final states!
    let yrp = rep.to_yrp_bytes().unwrap();
    assert_eq!(u32::from_le_bytes(yrp[0..4].try_into().unwrap()), YRP_MAGIC);
    let (_hdr, rep_from_yrp) = Replay::from_yrp_bytes(&yrp).unwrap();

    let yrpx = rep.to_yrpx_bytes().unwrap();
    assert_eq!(u32::from_le_bytes(yrpx[0..4].try_into().unwrap()), YRPX_MAGIC);
    let rep_from_yrpx = Replay::from_yrpx_bytes(&yrpx).unwrap();

    let om = rep.to_omegarp_bytes().unwrap();
    let rep_from_om = Replay::from_omegarp_bytes(&om).unwrap();

    let player = ReplayPlayer::load(rep_from_yrp, &db).unwrap();
    assert_eq!(player.final_state().players[0].lp, eng.get_lp(0));
    assert_eq!(player.final_state().players[1].lp, eng.get_lp(1));
    assert_eq!(player.final_state().winner, eng.state.winner);
    assert_eq!(rep_from_yrpx.actions.len(), rep_from_om.actions.len());
}

#[tokio::test]
async fn test_multiplayer_server_players_spectator_chat_and_ranked_queue() {
    let db = CardDatabase::with_builtin();
    let decks = db.starter_decks();
    let server = OmegaServer::new(db.clone());
    let (addr, handle) = server.bind("127.0.0.1:0").await.unwrap();

    let mut c1 = OmegaClient::connect(addr).await.unwrap();
    let mut c2 = OmegaClient::connect(addr).await.unwrap();
    let mut spec = OmegaClient::connect(addr).await.unwrap();

    c1.send(&ClientPacket::Hello { player_name: "Yugi".into() }).await.unwrap();
    c2.send(&ClientPacket::Hello { player_name: "Kaiba".into() }).await.unwrap();
    spec.send(&ClientPacket::Hello { player_name: "JoeySpec".into() }).await.unwrap();
    assert!(matches!(c1.recv().await.unwrap(), ServerPacket::Welcome { .. }));
    assert!(matches!(c2.recv().await.unwrap(), ServerPacket::Welcome { .. }));
    assert!(matches!(spec.recv().await.unwrap(), ServerPacket::Welcome { .. }));

    // Create passcode-protected room
    c1.send(&ClientPacket::CreateRoom {
        title: "Battle City Finals".into(),
        passcode: Some("BLUEEYES".into()),
        format: FormatPreset::TcgAdvanced,
        mode: RoomMode::Single,
        manual_mode: true,
    }).await.unwrap();
    let room_id = match c1.recv().await.unwrap() {
        ServerPacket::RoomJoined { room, seat: Some(0) } => room.room_id,
        other => panic!("expected RoomJoined, got {other:?}"),
    };

    // Wrong passcode rejected!
    c2.send(&ClientPacket::JoinRoom { room_id: room_id.clone(), passcode: Some("WRONG".into()), as_spectator: false }).await.unwrap();
    assert!(matches!(c2.recv().await.unwrap(), ServerPacket::Error(_)));

    // Right passcode joins Seat 1 + Spectator joins!
    c2.send(&ClientPacket::JoinRoom { room_id: room_id.clone(), passcode: Some("BLUEEYES".into()), as_spectator: false }).await.unwrap();
    assert!(matches!(c2.recv().await.unwrap(), ServerPacket::RoomJoined { seat: Some(1), .. }));
    let _ = c1.recv().await.unwrap(); // RoomUpdated
    let _ = c2.recv().await.unwrap(); // RoomUpdated

    spec.send(&ClientPacket::JoinRoom { room_id: room_id.clone(), passcode: Some("BLUEEYES".into()), as_spectator: true }).await.unwrap();
    assert!(matches!(spec.recv().await.unwrap(), ServerPacket::RoomJoined { seat: None, .. }));
    let _ = c1.recv().await.unwrap();
    let _ = c2.recv().await.unwrap();

    // Both submit decks & Ready -> Live Duel starts & syncs masked state to P1, P2, AND Spectator!
    c1.send(&ClientPacket::SubmitDeck(decks[0].clone())).await.unwrap();
    c2.send(&ClientPacket::SubmitDeck(decks[1].clone())).await.unwrap();
    c1.send(&ClientPacket::Ready(true)).await.unwrap();
    c2.send(&ClientPacket::Ready(true)).await.unwrap();

    let s1 = c1.recv().await.unwrap();
    let s2 = c2.recv().await.unwrap();
    let ss = spec.recv().await.unwrap();
    assert!(matches!(s1, ServerPacket::DuelStateSync { .. }));
    assert!(matches!(s2, ServerPacket::DuelStateSync { .. }));
    assert!(matches!(ss, ServerPacket::DuelStateSync { .. }));

    // Verify spectator receives MASKED hidden hands (code == 0)!
    if let ServerPacket::DuelStateSync { state, .. } = ss {
        let hand_cards: Vec<_> = state.cards.values().filter(|c| c.location == Location::HAND).collect();
        assert!(!hand_cards.is_empty());
        assert!(hand_cards.iter().all(|c| c.data.code == 0));
    }

    // Chat broadcast to players & spectator
    c1.send(&ClientPacket::Chat("It's time to duel!".into())).await.unwrap();
    assert!(matches!(c1.recv().await.unwrap(), ServerPacket::Chat { .. }));
    assert!(matches!(c2.recv().await.unwrap(), ServerPacket::Chat { .. }));
    assert!(matches!(spec.recv().await.unwrap(), ServerPacket::Chat { .. }));

    handle.abort();
}

#[test]
fn test_ranked_elo_single_elim_swiss_and_draft_cube() {
    let db = CardDatabase::with_builtin();

    // Ranked Ladder & Tiers
    let mut ladder = RankedLadder::new(1);
    for i in 0..25 {
        ladder.record_match(FormatPreset::TcgAdvanced, "Champion", &format!("Rival-{i}"), 1.0);
    }
    let champ = ladder.leaderboard(FormatPreset::TcgAdvanced, 1);
    assert_eq!(champ[0].player, "Champion");
    assert!(champ[0].tier() >= RankTier::Gold);
    assert!(ladder.enqueue(FormatPreset::TcgAdvanced, "QueueA", 300).is_none());
    assert!(ladder.enqueue(FormatPreset::TcgAdvanced, "QueueB", 300).is_some());

    // Single-Elimination 4-player bracket
    let mut bracket = Tournament::new("ELIM-1", "YCS Top 4", FormatPreset::TcgAdvanced, TournamentKind::SingleElimination);
    for p in ["P1", "P2", "P3", "P4"] { bracket.register(p); }
    let semis = bracket.start_next_round().unwrap();
    assert_eq!(semis.len(), 2);
    bracket.report_result(1, MatchOutcome::PlayerAWins).unwrap();
    bracket.report_result(2, MatchOutcome::PlayerBWins).unwrap();
    let finals = bracket.start_next_round().unwrap();
    assert_eq!(finals.len(), 1);
    bracket.report_result(1, MatchOutcome::PlayerAWins).unwrap();
    assert!(bracket.finished);
    assert_eq!(bracket.champion.as_deref(), Some("P1"));

    // Sealed & Draft Cube
    let cube: Vec<CardCode> = db.cards.keys().copied().collect();
    let sealed = DraftSession::generate_sealed_pool(&cube, 6, 15, 123);
    assert_eq!(sealed.len(), 90);

    let mut draft = DraftSession::new(vec!["A".into(), "B".into(), "C".into(), "D".into()], cube, 3, 15, 99);
    for _ in 0..(3 * 15) {
        for seat in 0..4 {
            draft.pick(seat, 0).unwrap();
        }
    }
    assert!(draft.finished);
    assert_eq!(draft.pools[0].len(), 45);
    let built = draft.build_deck_for_seat(0, &db);
    assert!(!built.main.is_empty());

    // Verify frame codec & sandbox & hypergeometric & match flow
    let f = encode_frame(ctos::RESPONSE, &[1, 2, 3]);
    assert_eq!(decode_frame(&f).unwrap().0, ctos::RESPONSE);
    let mut sb = HandTestSandbox::new(db.starter_decks()[0].clone(), FormatPreset::TcgAdvanced, 55, &db);
    assert_eq!(sb.hand_names().len(), 5);
    sb.redraw_opening_hand(56, &db);
    assert_eq!(sb.hand_names().len(), 5);
    assert!((hypergeometric_at_least(40, 12, 5, 1) - 0.8476).abs() < 0.01);

    let mut ms = MatchSession::new(true, db.starter_decks()[0].clone(), db.starter_decks()[1].clone());
    ms.submit_rps(0, RpsMove::Paper).unwrap();
    ms.submit_rps(1, RpsMove::Paper).unwrap(); // Tie resets RPS!
    assert_eq!(ms.phase, MatchPhase::WaitingRps);
    ms.submit_rps(0, RpsMove::Paper).unwrap();
    ms.submit_rps(1, RpsMove::Rock).unwrap();
    assert_eq!(ms.phase, MatchPhase::WaitingTurnOrder { chooser: 0 });
    let _ = mask_state_for_viewer(&sb.engine.state, Some(0), false);
}

#[test]
fn test_full_25k_card_database_and_official_lua_scripts() {
    let db = CardDatabase::load_default();
    assert!(
        db.len() >= 25_000,
        "expected full 25,000+ card database from data/cdb, got {}",
        db.len()
    );
    assert!(db.count_available_scripts() >= 22_000);
    assert!(db.loaded_banlists.len() >= 8);

    // Spot-check cards across every era & format (TCG/OCG modern, GOAT, Rush, Speed, Anime)
    assert_eq!(db.get(89631139).unwrap().name, "Blue-Eyes White Dragon");
    assert!(db.get(32909498).unwrap().name.contains("Kashtira Fenrir"));
    assert!(db.get(92731385).unwrap().name.contains("Kitkallos"));
    assert!(db.get(70368879).unwrap().name.contains("Upstart Goblin"));
    assert!(db.get(160001001).is_some()); // Rush Duel card

    // Load ALL 22 ProjectIgnis core Lua libraries into our pure-Rust Lua 5.4 VM with 0 errors
    let mut host = LuaScriptHost::new();
    db.attach_to_script_host(&mut host);
    let libs = host.load_core_libraries().unwrap();
    assert_eq!(libs, 22);
    assert!(host.vm.output_log.is_empty(), "core lib errors: {:?}", host.vm.output_log);

    // Execute real official ProjectIgnis scripts directly (Kashtira Fenrir, Tearlaments Kitkallos, Branded Fusion, Accesscode Talker, Baronne de Fleur, ZEUS!)
    let mut raw_host = LuaScriptHost::new();
    for dir in &db.script_dirs {
        raw_host.add_script_dir(dir.clone());
    }
    if let Some(ref root) = db.data_root {
        raw_host.set_core_script_dir(root.join("scripts"));
    }
    assert_eq!(raw_host.load_core_libraries().unwrap(), 22);
    let mut eng = DuelEngine::new(DuelConfig::from_preset(FormatPreset::TcgAdvanced, 1), "A", "B");
    for code in [32909498, 92731385, 44362883, 86066372, 84815190, 90448279, 14558127, 10045474] {
        let cd = db.get(code).unwrap().clone();
        let cid = eng.add_card(cd, 0, Location::HAND, Position::FACEUP_ATTACK, &mut raw_host);
        assert!(
            !eng.card(cid).unwrap().effects.is_empty(),
            "official script c{code}.lua should register effects"
        );
    }
    assert!(
        raw_host.vm.output_log.is_empty(),
        "unexpected official script errors: {:?}",
        raw_host.vm.output_log
    );
}
