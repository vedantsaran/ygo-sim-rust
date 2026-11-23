use neon_ygo_core::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiDifficulty {
    Casual,
    Competitive,
}

/// Strategic heuristic Yu-Gi-Oh! Duel AI (offline Solo / vs AI mode matching YGO Omega).
#[derive(Debug, Clone)]
pub struct DuelAi {
    pub name: String,
    pub difficulty: AiDifficulty,
}

impl DuelAi {
    pub fn new(name: impl Into<String>, difficulty: AiDifficulty) -> Self {
        Self {
            name: name.into(),
            difficulty,
        }
    }

    /// Evaluate the current board score from `player`'s perspective.
    pub fn evaluate_board(&self, engine: &DuelEngine, player: PlayerId) -> i32 {
        let opp = engine.opponent_of(player);
        let lp_diff = (engine.get_lp(player) - engine.get_lp(opp)) / 10;
        let my_atk: i32 = engine
            .cards_in(player, Location::MZONE)
            .into_iter()
            .filter_map(|id| engine.card(id))
            .map(|c| c.current_atk() / 5 + 150)
            .sum();
        let opp_atk: i32 = engine
            .cards_in(opp, Location::MZONE)
            .into_iter()
            .filter_map(|id| engine.card(id))
            .map(|c| c.current_atk() / 5 + 150)
            .sum();
        let hand_diff = (engine.cards_in(player, Location::HAND).len() as i32
            - engine.cards_in(opp, Location::HAND).len() as i32)
            * 120;
        lp_diff + (my_atk - opp_atk) + hand_diff
    }

    /// Choose the best legal `Decision` for `engine`'s active prompt.
    pub fn choose_decision(&self, engine: &mut DuelEngine, host: &mut dyn ScriptHost) -> Option<Decision> {
        let prompt = engine.current_prompt(host)?;
        match prompt {
            Prompt::SelectIdleCmd {
                player,
                summonable,
                activatable,
                ssetable,
                maximum_summonable,
                can_battle_phase,
                ..
            } => {
                let opp = engine.opponent_of(player);

                // 1. Maximum Summon if available in Rush Duel!
                if let Some(&pieces) = maximum_summonable.first() {
                    return Some(Decision::MaximumSummon { pieces });
                }

                // 2. Activate high-value Spells / Effects (Draws, Raigeki when opp has monsters, Monster Reborn, MST)
                for opt in &activatable {
                    if let Some(c) = engine.card(opt.card_id) {
                        let code = c.data.code;
                        // Avoid blowing up our own stronger board with Dark Hole
                        if code == 53129443 {
                            let my_cnt = engine.cards_in(player, Location::MZONE).len();
                            let opp_cnt = engine.cards_in(opp, Location::MZONE).len();
                            if my_cnt >= opp_cnt {
                                continue;
                            }
                        }
                        if c.data.is_spell() && !c.data.card_type.contains(CardType::EQUIP) {
                            return Some(Decision::ActivateEffect {
                                card_id: opt.card_id,
                                effect_id: opt.effect_id,
                                targets: Vec::new(),
                            });
                        }
                    }
                }

                // 3. Extra Deck Xyz / Synchro / Link summons when legal materials exist
                if let Some(ed_dec) = self.try_extra_deck_summon(engine, player) {
                    return Some(ed_dec);
                }

                // 4. Normal / Tribute Summon the highest-ATK monster we can legally summon!
                let mut best_summon: Option<(i32, CardId, Vec<CardId>)> = None;
                let my_mons = engine.cards_in(player, Location::MZONE);
                for &cid in &summonable {
                    if let Some(c) = engine.card(cid) {
                        let req = c.data.required_tributes() as usize;
                        if req == 0 {
                            let atk = c.data.atk;
                            if best_summon.as_ref().map(|b| atk > b.0).unwrap_or(true) {
                                best_summon = Some((atk, cid, Vec::new()));
                            }
                        } else if my_mons.len() >= req {
                            // Sort controlled monsters by ascending ATK to pick cheapest tributes
                            let mut sorted: Vec<(i32, CardId)> = my_mons
                                .iter()
                                .filter_map(|&mid| engine.card(mid).map(|m| (m.current_atk(), mid)))
                                .collect();
                            sorted.sort_by_key(|&(a, _)| a);
                            let trib_atk_sum: i32 = sorted.iter().take(req).map(|&(a, _)| a).sum();
                            if c.data.atk > trib_atk_sum {
                                let tribs = sorted.into_iter().take(req).map(|(_, id)| id).collect();
                                if best_summon.as_ref().map(|b| c.data.atk > b.0).unwrap_or(true) {
                                    best_summon = Some((c.data.atk, cid, tribs));
                                }
                            }
                        }
                    }
                }
                if let Some((_atk, cid, tributes)) = best_summon {
                    return Some(Decision::NormalSummon {
                        card_id: cid,
                        tributes,
                        zone: None,
                    });
                }

                // 5. Enter Battle Phase if we have an attacker that can deal damage or win a battle!
                if can_battle_phase && self.should_enter_battle(engine, player) {
                    return Some(Decision::EnterBattlePhase);
                }

                // 6. Set defensive Traps in Main Phase (if S/T slots free and not already flooding)
                let st_count = engine.cards_in(player, Location::SZONE).len();
                if st_count < 2 {
                    for &cid in &ssetable {
                        if let Some(c) = engine.card(cid) {
                            if c.data.is_trap() {
                                return Some(Decision::SetSpellTrap { card_id: cid, zone: None });
                            }
                        }
                    }
                }

                Some(Decision::EndTurn)
            }
            Prompt::SelectBattleCmd {
                attackable,
                can_main2,
                ..
            } => {
                // Pick the most profitable attack: direct attack or strictly winning combat!
                let mut best_atk: Option<(i32, CardId, Option<CardId>)> = None;
                for (attacker_id, targets) in attackable {
                    let atk_val = engine.card(attacker_id).map(|c| c.current_atk()).unwrap_or(0);
                    for target in targets {
                        let score = match target {
                            None => atk_val + 1000, // Direct attack!
                            Some(tid) => {
                                let Some(def_c) = engine.card(tid) else { continue };
                                if def_c.is_facedown() {
                                    if atk_val >= 1500 { atk_val } else { -500 }
                                } else if def_c.is_attack_pos() {
                                    let datk = def_c.current_atk();
                                    if atk_val > datk { (atk_val - datk) + 800 } else { -1000 }
                                } else {
                                    let ddef = def_c.current_def();
                                    if atk_val > ddef { 600 } else { -1000 }
                                }
                            }
                        };
                        if score > 0 && best_atk.as_ref().map(|b| score > b.0).unwrap_or(true) {
                            best_atk = Some((score, attacker_id, target));
                        }
                    }
                }
                if let Some((_, attacker, target)) = best_atk {
                    return Some(Decision::DeclareAttack { attacker, target });
                }
                if can_main2 && self.difficulty == AiDifficulty::Competitive {
                    Some(Decision::EnterMainPhase2)
                } else {
                    Some(Decision::EndTurn)
                }
            }
            Prompt::SelectChain { .. } => Some(Decision::Pass),
            _ => Some(Decision::Pass),
        }
    }

    fn should_enter_battle(&self, engine: &DuelEngine, player: PlayerId) -> bool {
        let opp = engine.opponent_of(player);
        let opp_mons = engine.cards_in(opp, Location::MZONE);
        let my_mons: Vec<&CardInstance> = engine
            .cards_in(player, Location::MZONE)
            .into_iter()
            .filter_map(|id| engine.card(id))
            .filter(|c| c.is_faceup() && c.is_attack_pos() && !c.computed.cannot_attack)
            .collect();
        if my_mons.is_empty() {
            return false;
        }
        if opp_mons.is_empty() {
            return my_mons.iter().any(|m| m.current_atk() > 0);
        }
        let min_opp_stat = opp_mons
            .iter()
            .filter_map(|&id| engine.card(id))
            .map(|c| if c.is_attack_pos() { c.current_atk() } else { c.current_def() })
            .min()
            .unwrap_or(0);
        my_mons.iter().any(|m| m.current_atk() > min_opp_stat)
    }

    fn try_extra_deck_summon(&self, engine: &DuelEngine, player: PlayerId) -> Option<Decision> {
        let my_mons: Vec<&CardInstance> = engine
            .cards_in(player, Location::MZONE)
            .into_iter()
            .filter_map(|id| engine.card(id))
            .filter(|c| c.is_faceup())
            .collect();
        if my_mons.len() < 2 {
            return None;
        }
        let extra = engine.cards_in(player, Location::EXTRA);
        for ex_id in extra {
            let ex = engine.card(ex_id)?;
            if engine.valid_extra_deck_zones(player, &ex.data).is_empty() {
                continue;
            }
            // Try Xyz Summon (2 matching Level monsters with lower ATK than Xyz)
            if ex.data.is_xyz() {
                let matching: Vec<CardId> = my_mons
                    .iter()
                    .filter(|m| !m.data.card_type.contains(CardType::TOKEN) && m.computed.level == ex.data.level)
                    .map(|m| m.id)
                    .collect();
                if matching.len() >= 2 {
                    let max_mat_atk = matching.iter().take(2).filter_map(|id| engine.card(*id)).map(|c| c.current_atk()).max().unwrap_or(0);
                    if ex.data.atk > max_mat_atk {
                        return Some(Decision::SpecialSummon {
                            card_id: ex_id,
                            materials: matching[..2].to_vec(),
                            zone: None,
                            position: Some(Position::FACEUP_ATTACK),
                        });
                    }
                }
            }
            // Try Synchro Summon (1 Tuner + 1 non-Tuner summing to exact Level)
            if ex.data.card_type.contains(CardType::SYNCHRO) {
                for t in my_mons.iter().filter(|m| m.data.is_tuner()) {
                    for nt in my_mons.iter().filter(|m| !m.data.is_tuner() && !m.data.is_xyz() && !m.data.is_link()) {
                        if t.computed.level + nt.computed.level == ex.data.level {
                            return Some(Decision::SpecialSummon {
                                card_id: ex_id,
                                materials: vec![t.id, nt.id],
                                zone: None,
                                position: Some(Position::FACEUP_ATTACK),
                            });
                        }
                    }
                }
            }
        }
        None
    }

    /// Run a complete automated AI vs AI duel up to `max_steps` decisions (returns winner & total actions).
    pub fn run_ai_vs_ai(
        engine: &mut DuelEngine,
        host: &mut dyn ScriptHost,
        ai0: &DuelAi,
        ai1: &DuelAi,
        max_steps: usize,
    ) -> (Option<Option<PlayerId>>, usize) {
        let mut steps = 0usize;
        while !engine.is_finished() && steps < max_steps {
            let ai = if engine.state.turn_player % 2 == 0 { ai0 } else { ai1 };
            let Some(dec) = ai.choose_decision(engine, host) else {
                break;
            };
            if engine.apply_decision(dec, host).is_err() {
                let _ = engine.apply_decision(Decision::EndTurn, host);
            }
            steps += 1;
        }
        (engine.state.winner, steps)
    }
}
