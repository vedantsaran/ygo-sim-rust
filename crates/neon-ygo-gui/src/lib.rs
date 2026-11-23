use eframe::egui;
use serde::{Deserialize, Serialize};

use neon_ygo_ai::{builtin_puzzles, AiDifficulty, DuelAi, DuelPuzzle, HandTestSandbox, PuzzleSession};
use neon_ygo_core::*;
use neon_ygo_db::{hypergeometric_at_least, Banlist, CardDatabase, Deck, DeckFilter, SortBy};
use neon_ygo_lua::LuaScriptHost;
use neon_ygo_net::{DraftSession, RankedLadder, Tournament, TournamentKind};
use neon_ygo_replay::{Replay, ReplayPlayer};

/// YGO Omega-style Player Customization Profile (Sleeves, Playmats, Field Themes, Chain Defaults, Audio).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomizationProfile {
    pub duelist_name: String,
    pub avatar: String,
    pub playmat_theme: PlaymatTheme,
    pub card_sleeve: SleeveStyle,
    pub default_chain_mode: ChainControlMode,
    pub bgm_volume: u8,
    pub sfx_volume: u8,
}

impl Default for CustomizationProfile {
    fn default() -> Self {
        Self {
            duelist_name: "Neon Duelist".into(),
            avatar: "Blue-Eyes".into(),
            playmat_theme: PlaymatTheme::CyberNeon,
            card_sleeve: SleeveStyle::MillenniumGold,
            default_chain_mode: ChainControlMode::Auto,
            bgm_volume: 70,
            sfx_volume: 85,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlaymatTheme {
    CyberNeon,
    DuelistKingdom,
    BattleCity,
    VrainsLink,
    ShadowRealm,
}

impl PlaymatTheme {
    pub fn bg_color(self) -> egui::Color32 {
        match self {
            PlaymatTheme::CyberNeon => egui::Color32::from_rgb(13, 18, 30),
            PlaymatTheme::DuelistKingdom => egui::Color32::from_rgb(20, 34, 24),
            PlaymatTheme::BattleCity => egui::Color32::from_rgb(28, 22, 34),
            PlaymatTheme::VrainsLink => egui::Color32::from_rgb(10, 26, 42),
            PlaymatTheme::ShadowRealm => egui::Color32::from_rgb(24, 12, 22),
        }
    }

    pub fn accent_color(self) -> egui::Color32 {
        match self {
            PlaymatTheme::CyberNeon => egui::Color32::from_rgb(56, 189, 248),
            PlaymatTheme::DuelistKingdom => egui::Color32::from_rgb(74, 222, 128),
            PlaymatTheme::BattleCity => egui::Color32::from_rgb(250, 204, 21),
            PlaymatTheme::VrainsLink => egui::Color32::from_rgb(34, 211, 238),
            PlaymatTheme::ShadowRealm => egui::Color32::from_rgb(244, 63, 94),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SleeveStyle {
    MillenniumGold,
    DarkMagicianPurple,
    BlueEyesSilver,
    CyberCircuit,
    ClassicBack,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveTab {
    DuelArena,
    DeckBuilder,
    Puzzles,
    HandTest,
    TournamentsAndRanked,
    Replays,
    Customize,
}

/// Complete Pure-Rust Native Desktop GUI Client (1:1 YGO Omega Client).
pub struct NeonYgoApp {
    tab: ActiveTab,
    db: CardDatabase,
    profile: CustomizationProfile,
    selected_format: FormatPreset,

    // Active Duel State
    engine: DuelEngine,
    host: LuaScriptHost,
    ai: DuelAi,
    inspected_card: Option<CardId>,
    active_replay: Replay,
    duel_log: Vec<String>,
    show_judge_drawer: bool,
    manual_lp_input: String,

    // Deck Builder State
    decks: Vec<Deck>,
    active_deck_idx: usize,
    filter_text: String,
    filter_type: u8, // 0=All, 1=Monster, 2=Spell, 3=Trap, 4=Extra
    import_export_buf: String,
    deck_status_msg: String,
    prob_copies: usize,
    prob_hand: usize,
    prob_min: usize,

    // Puzzle State
    puzzles: Vec<DuelPuzzle>,
    active_puzzle: Option<PuzzleSession>,

    // Hand Test State
    sandbox: Option<HandTestSandbox>,
    sandbox_seed: u32,

    // Ranked, Tournament & Draft State
    ranked: RankedLadder,
    tournament: Tournament,
    draft: DraftSession,

    // Replay Viewer State
    replay_player: Option<ReplayPlayer>,
}

impl NeonYgoApp {
    pub fn new() -> Self {
        let db = CardDatabase::load_default();
        let decks = db.starter_decks();
        let profile = CustomizationProfile::default();
        let selected_format = FormatPreset::TcgAdvanced;

        let (engine, host, active_replay) = Self::create_duel(&db, &decks[0], &decks[1], selected_format, 42);

        let mut ranked = RankedLadder::new(1);
        ranked.record_match(FormatPreset::TcgAdvanced, "Neon Duelist", "KaibaBot", 1.0);
        ranked.record_match(FormatPreset::TcgAdvanced, "YugiBot", "JoeyBot", 1.0);
        ranked.record_match(FormatPreset::TcgAdvanced, "Neon Duelist", "YugiBot", 1.0);

        let mut tournament = Tournament::new(
            "T-001",
            "Omega Weekend Swiss",
            FormatPreset::TcgAdvanced,
            TournamentKind::Swiss { rounds: 3 },
        );
        for p in ["Neon Duelist", "Yugi", "Kaiba", "Joey", "Pegasus", "Marik"] {
            tournament.register(p);
        }
        let _ = tournament.start_next_round();

        let cube: Vec<CardCode> = db.cards.keys().copied().collect();
        let draft = DraftSession::new(
            vec!["Neon Duelist".into(), "Bot A".into(), "Bot B".into(), "Bot C".into()],
            cube,
            3,
            15,
            777,
        );

        Self {
            tab: ActiveTab::DuelArena,
            db,
            profile,
            selected_format,
            engine,
            host,
            ai: DuelAi::new("Omega AI", AiDifficulty::Competitive),
            inspected_card: None,
            active_replay,
            duel_log: vec!["Duel initialized! Your turn (Turn 1 Main Phase 1).".into()],
            show_judge_drawer: false,
            manual_lp_input: "8000".into(),
            decks,
            active_deck_idx: 0,
            filter_text: String::new(),
            filter_type: 0,
            import_export_buf: String::new(),
            deck_status_msg: String::new(),
            prob_copies: 3,
            prob_hand: 5,
            prob_min: 1,
            puzzles: builtin_puzzles(),
            active_puzzle: None,
            sandbox: None,
            sandbox_seed: 100,
            ranked,
            tournament,
            draft,
            replay_player: None,
        }
    }

    fn create_duel(
        db: &CardDatabase,
        deck0: &Deck,
        deck1: &Deck,
        format: FormatPreset,
        seed: u32,
    ) -> (DuelEngine, LuaScriptHost, Replay) {
        let cfg = DuelConfig::from_preset(format, seed);
        let mut engine = DuelEngine::new(cfg.clone(), "You", "Omega AI");
        let mut host = LuaScriptHost::new();
        db.attach_to_script_host(&mut host);
        let _ = host.load_core_libraries();

        let (m0, e0, s0) = deck0.resolve(db);
        let (m1, e1, s1) = deck1.resolve(db);
        engine.load_deck(0, m0, e0, s0, &mut host);
        engine.load_deck(1, m1, e1, s1, &mut host);
        engine.start_duel(0, &mut host);

        let replay = Replay::new(
            format!("{format:?} vs Omega AI"),
            cfg,
            0,
            vec![("You".into(), deck0.clone()), ("Omega AI".into(), deck1.clone())],
        );
        (engine, host, replay)
    }

    fn restart_duel(&mut self) {
        let d0 = self.decks.get(self.active_deck_idx).cloned().unwrap_or_default();
        let d1 = self.decks.get((self.active_deck_idx + 1) % self.decks.len().max(1)).cloned().unwrap_or_else(|| d0.clone());
        let seed = self.engine.state.config.seed.wrapping_add(1337);
        let (eng, host, rep) = Self::create_duel(&self.db, &d0, &d1, self.selected_format, seed);
        self.engine = eng;
        self.host = host;
        self.active_replay = rep;
        self.engine.set_chain_control(0, self.profile.default_chain_mode);
        self.duel_log.clear();
        self.duel_log.push(format!("Started new {:?} duel!", self.selected_format));
    }

    fn apply_player_decision(&mut self, dec: Decision) {
        self.active_replay.record_decision(dec.clone());
        match self.engine.apply_decision(dec, &mut self.host) {
            Ok(()) => {
                for m in self.engine.drain_messages() {
                    self.duel_log.push(format!("{m:?}"));
                }
                self.step_ai_if_opponent_turn();
            }
            Err(e) => self.duel_log.push(format!("Illegal: {e}")),
        }
    }

    fn step_ai_if_opponent_turn(&mut self) {
        let mut guard = 0;
        while !self.engine.is_finished() && self.engine.state.turn_player == 1 && guard < 40 {
            guard += 1;
            let Some(dec) = self.ai.choose_decision(&mut self.engine, &mut self.host) else {
                break;
            };
            self.active_replay.record_decision(dec.clone());
            if self.engine.apply_decision(dec, &mut self.host).is_err() {
                let _ = self.engine.apply_decision(Decision::EndTurn, &mut self.host);
            }
            for m in self.engine.drain_messages() {
                self.duel_log.push(format!("[AI] {m:?}"));
            }
        }
    }
}

impl Default for NeonYgoApp {
    fn default() -> Self {
        Self::new()
    }
}

impl eframe::App for NeonYgoApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Omega Chain Control Hotkeys: Tab cycles Auto/All/Legal/Off
        if ctx.input(|i| i.key_pressed(egui::Key::Tab)) && self.tab == ActiveTab::DuelArena {
            let next = match self.engine.chain_control(0) {
                ChainControlMode::Auto => ChainControlMode::All,
                ChainControlMode::All => ChainControlMode::Legal,
                ChainControlMode::Legal => ChainControlMode::Off,
                ChainControlMode::Off => ChainControlMode::Auto,
            };
            self.engine.set_chain_control(0, next);
        }

        let theme_bg = self.profile.playmat_theme.bg_color();
        let accent = self.profile.playmat_theme.accent_color();

        egui::TopBottomPanel::top("nav_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.colored_label(accent, egui::RichText::new("NEON YGO SIM").strong().size(18.0));
                ui.separator();
                ui.selectable_value(&mut self.tab, ActiveTab::DuelArena, "Duel Arena");
                ui.selectable_value(&mut self.tab, ActiveTab::DeckBuilder, "Deck Builder");
                ui.selectable_value(&mut self.tab, ActiveTab::Puzzles, "Puzzles");
                ui.selectable_value(&mut self.tab, ActiveTab::HandTest, "Hand Test");
                ui.selectable_value(&mut self.tab, ActiveTab::TournamentsAndRanked, "Ranked & Tournaments");
                ui.selectable_value(&mut self.tab, ActiveTab::Replays, "Replays");
                ui.selectable_value(&mut self.tab, ActiveTab::Customize, "Customize");
            });
        });

        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(&ctx.style()).fill(theme_bg))
            .show(ctx, |ui| match self.tab {
                ActiveTab::DuelArena => self.ui_duel_arena(ui, accent),
                ActiveTab::DeckBuilder => self.ui_deck_builder(ui, accent),
                ActiveTab::Puzzles => self.ui_puzzles(ui, accent),
                ActiveTab::HandTest => self.ui_hand_test(ui, accent),
                ActiveTab::TournamentsAndRanked => self.ui_tournaments_ranked(ui, accent),
                ActiveTab::Replays => self.ui_replays(ui, accent),
                ActiveTab::Customize => self.ui_customize(ui, accent),
            });
    }
}

impl NeonYgoApp {
    fn ui_duel_arena(&mut self, ui: &mut egui::Ui, accent: egui::Color32) {
        // Top HUD: Format selector, LP bars, Phase stepper, Chain Control, Manual Toggle
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("duel_format")
                .selected_text(format!("{:?}", self.selected_format))
                .show_ui(ui, |ui| {
                    for fmt in [
                        FormatPreset::TcgAdvanced,
                        FormatPreset::Ocg,
                        FormatPreset::MasterDuel,
                        FormatPreset::Goat,
                        FormatPreset::Edison,
                        FormatPreset::SpeedDuel,
                        FormatPreset::RushDuel,
                        FormatPreset::TurboDuel,
                        FormatPreset::TagDuel,
                        FormatPreset::RelayDuel,
                        FormatPreset::Genesys,
                    ] {
                        ui.selectable_value(&mut self.selected_format, fmt, format!("{fmt:?}"));
                    }
                });
            if ui.button("New Duel").clicked() {
                self.restart_duel();
            }
            if ui.button("Save Replay").clicked() {
                if let Ok(rp) = ReplayPlayer::load(self.active_replay.clone(), &self.db) {
                    self.replay_player = Some(rp);
                    self.duel_log.push("Saved current duel to Replay Viewer!".into());
                }
            }
            ui.separator();
            ui.label(format!(
                "Turn {} | {} | Phase: {:?}",
                self.engine.state.turn_count,
                if self.engine.state.turn_player == 0 { "YOUR TURN" } else { "AI TURN" },
                self.engine.state.phase
            ));
            ui.separator();
            // Omega 4-Mode Chain Control Bar!
            ui.label("Chain:");
            let mut cm = self.engine.chain_control(0);
            for (mode, label) in [
                (ChainControlMode::Auto, "AUTO"),
                (ChainControlMode::All, "ALL"),
                (ChainControlMode::Legal, "LEGAL"),
                (ChainControlMode::Off, "OFF"),
            ] {
                if ui.selectable_label(cm == mode, label).clicked() {
                    cm = mode;
                    self.engine.set_chain_control(0, mode);
                }
            }
            ui.separator();
            let mut manual = self.engine.state.manual_mode;
            if ui.checkbox(&mut manual, "Manual/Judge").changed() {
                let _ = self.engine.apply_manual_command(ManualCommand::SetManualMode(manual), &mut self.host);
                self.show_judge_drawer = manual;
            }
        });

        ui.separator();

        // LP Bars
        ui.horizontal(|ui| {
            let lp0 = self.engine.get_lp(0);
            let lp1 = self.engine.get_lp(1);
            let max_lp = self.engine.state.config.starting_lp.max(1) as f32;
            ui.colored_label(accent, format!("{}: {} LP", self.profile.duelist_name, lp0));
            ui.add(egui::ProgressBar::new((lp0 as f32 / max_lp).clamp(0.0, 1.0)).desired_width(180.0));
            ui.add_space(24.0);
            ui.colored_label(egui::Color32::from_rgb(248, 113, 113), format!("Omega AI: {} LP", lp1));
            ui.add(egui::ProgressBar::new((lp1 as f32 / max_lp).clamp(0.0, 1.0)).desired_width(180.0));
            if let Some(winner) = self.engine.state.winner {
                ui.colored_label(
                    egui::Color32::GOLD,
                    format!("DUEL ENDED! Winner: {:?} ({:?})", winner, self.engine.state.win_reason),
                );
            }
        });

        ui.separator();

        ui.columns(2, |cols| {
            // LEFT COLUMN: Battlefield + Hand + Phase Controls
            let ui = &mut cols[0];
            let mmz_n = self.engine.state.config.mmz_count() as u8;

            // Opponent Monster Zones
            ui.label(format!(
                "Opponent Field (Hand: {}, Deck: {}, GY: {})",
                self.engine.cards_in(1, Location::HAND).len(),
                self.engine.cards_in(1, Location::DECK).len(),
                self.engine.cards_in(1, Location::GRAVE).len(),
            ));
            ui.horizontal(|ui| {
                for seq in (0..mmz_n).rev() {
                    self.zone_card_button(ui, 1, Location::MZONE, seq, "OPP MMZ");
                }
            });

            // Shared Extra Monster Zones (EMZ Left & Right)
            if self.engine.state.config.has_emz() {
                ui.horizontal(|ui| {
                    ui.add_space(60.0);
                    self.zone_card_button(ui, 0, Location::MZONE, 5, "EMZ L");
                    ui.add_space(60.0);
                    self.zone_card_button(ui, 0, Location::MZONE, 6, "EMZ R");
                });
            }

            // Player Monster Zones
            ui.label("Your Monster Zones:");
            ui.horizontal(|ui| {
                for seq in 0..mmz_n {
                    self.zone_card_button(ui, 0, Location::MZONE, seq, "MMZ");
                }
            });

            // Player Spell/Trap & Pendulum Zones
            ui.label("Your Spell/Trap & Pendulum Zones:");
            ui.horizontal(|ui| {
                for seq in 0..mmz_n {
                    self.zone_card_button(ui, 0, Location::SZONE, seq, "S/T");
                }
            });

            ui.separator();

            // Action & Phase Buttons
            ui.horizontal(|ui| {
                if self.engine.can_enter_battle_phase(0) && ui.button("Battle Phase").clicked() {
                    self.apply_player_decision(Decision::EnterBattlePhase);
                }
                if self.engine.state.phase.is_battle_phase() && ui.button("Main Phase 2").clicked() {
                    self.apply_player_decision(Decision::EnterMainPhase2);
                }
                if ui.button("End Turn").clicked() {
                    self.apply_player_decision(Decision::EndTurn);
                }
                if ui.button("Surrender").clicked() {
                    self.apply_player_decision(Decision::Surrender(0));
                }
            });

            // Battle Phase Attack Commands
            if self.engine.state.phase.is_battle_phase() {
                ui.label("Declare Attacks:");
                let my_mons = self.engine.cards_in(0, Location::MZONE);
                for cid in my_mons {
                    let targets = self.engine.valid_attack_targets(cid);
                    if !targets.is_empty() {
                        let name = self.engine.card(cid).map(|c| c.data.name.clone()).unwrap_or_default();
                        ui.horizontal(|ui| {
                            ui.label(format!("{name} ->"));
                            for t in targets {
                                let tlabel = match t {
                                    None => "Direct Attack!".into(),
                                    Some(tid) => self.engine.card(tid).map(|c| c.data.name.clone()).unwrap_or_default(),
                                };
                                if ui.button(tlabel).clicked() {
                                    self.apply_player_decision(Decision::DeclareAttack { attacker: cid, target: t });
                                }
                            }
                        });
                    }
                }
            }

            ui.separator();

            // Player Hand Interactive Controls
            ui.label(format!(
                "Your Hand (Deck: {}, Extra: {}, GY: {}, Banished: {})",
                self.engine.cards_in(0, Location::DECK).len(),
                self.engine.cards_in(0, Location::EXTRA).len(),
                self.engine.cards_in(0, Location::GRAVE).len(),
                self.engine.cards_in(0, Location::REMOVED).len(),
            ));
            let hand = self.engine.cards_in(0, Location::HAND);
            let activatable = self.engine.list_activatable_options(0, &mut self.host);
            egui::ScrollArea::horizontal().show(ui, |ui| {
                ui.horizontal(|ui| {
                    for cid in hand {
                        let Some(c) = self.engine.card(cid).cloned() else { continue };
                        ui.group(|ui| {
                            ui.vertical(|ui| {
                                if ui.button(&c.data.name).clicked() {
                                    self.inspected_card = Some(cid);
                                }
                                if c.data.is_monster() {
                                    ui.label(format!("Lv{} {}/{}", c.computed.level, c.current_atk(), c.current_def()));
                                }
                                if self.engine.can_normal_summon(0, cid) && c.data.required_tributes() == 0 {
                                    if ui.button("Summon").clicked() {
                                        self.apply_player_decision(Decision::NormalSummon {
                                            card_id: cid,
                                            tributes: vec![],
                                            zone: None,
                                        });
                                    }
                                    if ui.button("Set").clicked() {
                                        self.apply_player_decision(Decision::SetMonster {
                                            card_id: cid,
                                            tributes: vec![],
                                            zone: None,
                                        });
                                    }
                                }
                                if let Some(opt) = activatable.iter().find(|o| o.card_id == cid) {
                                    if ui.button("Activate").clicked() {
                                        self.apply_player_decision(Decision::ActivateEffect {
                                            card_id: cid,
                                            effect_id: opt.effect_id,
                                            targets: vec![],
                                        });
                                    }
                                }
                                if (c.data.is_spell() || c.data.is_trap()) && self.engine.state.phase.is_main_phase() {
                                    if ui.button("Set S/T").clicked() {
                                        self.apply_player_decision(Decision::SetSpellTrap {
                                            card_id: cid,
                                            zone: None,
                                        });
                                    }
                                }
                            });
                        });
                    }
                });
            });

            // RIGHT COLUMN: Card Inspector, Manual/Judge Toolbox, Duel Log
            let ui = &mut cols[1];
            ui.heading("Card Inspector");
            if let Some(cid) = self.inspected_card {
                if let Some(c) = self.engine.card(cid) {
                    ui.colored_label(accent, egui::RichText::new(&c.data.name).strong().size(16.0));
                    ui.label(format!("Passcode: {} | Location: {:?} | Pos: {:?}", c.data.code, c.location, c.position));
                    if c.data.is_monster() {
                        ui.label(format!(
                            "ATK: {} (Base {}) | DEF: {} (Base {}) | Lv/Rk/Lk: {}",
                            c.current_atk(), c.computed.base_atk, c.current_def(), c.computed.base_def, c.data.level
                        ));
                    }
                    ui.label(&c.data.desc);
                }
            } else {
                ui.label("Click any card on the field or hand to inspect its live stats & text.");
            }

            if self.show_judge_drawer {
                ui.separator();
                ui.heading("Manual / Judge Toolbox");
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut self.manual_lp_input);
                    if ui.button("Set P1 LP").clicked() {
                        if let Ok(v) = self.manual_lp_input.parse::<i32>() {
                            let _ = self.engine.apply_manual_command(ManualCommand::SetLp { player: 0, lp: v }, &mut self.host);
                        }
                    }
                    if ui.button("Draw 1").clicked() {
                        let _ = self.engine.apply_manual_command(ManualCommand::DrawCards { player: 0, count: 1 }, &mut self.host);
                    }
                    if ui.button("Coin").clicked() {
                        let _ = self.engine.apply_manual_command(ManualCommand::TossCoin { player: 0, count: 1 }, &mut self.host);
                    }
                    if ui.button("Die").clicked() {
                        let _ = self.engine.apply_manual_command(ManualCommand::RollDice { player: 0, count: 1 }, &mut self.host);
                    }
                    if ui.button("Undo").clicked() {
                        let _ = self.engine.apply_manual_command(ManualCommand::Undo, &mut self.host);
                    }
                });
            }

            ui.separator();
            ui.heading("Duel Event Log");
            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                for line in self.duel_log.iter().rev().take(60) {
                    ui.label(line);
                }
            });
        });
    }

    fn zone_card_button(&mut self, ui: &mut egui::Ui, player: PlayerId, loc: Location, seq: u8, empty_label: &str) {
        if let Some(cid) = self.engine.card_at(player, loc, seq) {
            if let Some(c) = self.engine.card(cid) {
                let short: String = c.data.name.chars().take(12).collect();
                let txt = if c.data.is_monster() {
                    format!("{short}\n{}/{}", c.current_atk(), c.current_def())
                } else {
                    short
                };
                if ui.add_sized([92.0, 54.0], egui::Button::new(txt)).clicked() {
                    self.inspected_card = Some(cid);
                }
                return;
            }
        }
        let _ = ui.add_sized([92.0, 54.0], egui::Button::new(format!("{empty_label} {seq}")));
    }

    fn ui_deck_builder(&mut self, ui: &mut egui::Ui, accent: egui::Color32) {
        ui.horizontal(|ui| {
            ui.colored_label(accent, egui::RichText::new("Deck Builder & Legality Checker").strong().size(16.0));
            egui::ComboBox::from_id_salt("deck_sel")
                .selected_text(self.decks.get(self.active_deck_idx).map(|d| d.name.as_str()).unwrap_or("Deck"))
                .show_ui(ui, |ui| {
                    for (i, d) in self.decks.iter().enumerate() {
                        ui.selectable_value(&mut self.active_deck_idx, i, &d.name);
                    }
                });
            if ui.button("Export Omega Code").clicked() {
                if let Some(d) = self.decks.get(self.active_deck_idx) {
                    if let Ok(code) = d.to_omega_code() {
                        self.import_export_buf = code;
                        self.deck_status_msg = "Exported YGO Omega Deck Code!".into();
                    }
                }
            }
            if ui.button("Export ydke://").clicked() {
                if let Some(d) = self.decks.get(self.active_deck_idx) {
                    self.import_export_buf = d.to_ydke();
                    self.deck_status_msg = "Exported ydke:// link!".into();
                }
            }
            if ui.button("Import Omega/YDKE").clicked() {
                let buf = self.import_export_buf.trim().to_string();
                let res = if buf.starts_with("ydke://") {
                    Deck::from_ydke("Imported YDKE", &buf)
                } else {
                    Deck::from_omega_code("Imported Omega Deck", &buf, &self.db)
                };
                match res {
                    Ok(deck) => {
                        self.decks.push(deck);
                        self.active_deck_idx = self.decks.len() - 1;
                        self.deck_status_msg = "Successfully imported deck!".into();
                    }
                    Err(e) => self.deck_status_msg = format!("Import failed: {e}"),
                }
            }
        });

        ui.text_edit_singleline(&mut self.import_export_buf);
        if !self.deck_status_msg.is_empty() {
            ui.label(&self.deck_status_msg);
        }

        let bl = Banlist::preset(self.selected_format);
        let cfg = DuelConfig::from_preset(self.selected_format, 1);
        if let Some(deck) = self.decks.get(self.active_deck_idx) {
            let report = bl.validate_deck(deck, &self.db, &cfg);
            ui.horizontal(|ui| {
                ui.label(format!(
                    "Main: {} | Extra: {} | Side: {} | Status: {}",
                    deck.main.len(),
                    deck.extra.len(),
                    deck.side.len(),
                    if report.valid { "LEGAL" } else { "ILLEGAL" }
                ));
                if report.genesys_points_cap > 0 {
                    ui.label(format!(
                        "Genesys Points: {} / {}",
                        report.genesys_points_used, report.genesys_points_cap
                    ));
                }
            });
            for err in &report.errors {
                ui.colored_label(egui::Color32::from_rgb(248, 113, 113), err);
            }
        }

        ui.separator();

        // Hypergeometric Hand Probability Calculator
        ui.horizontal(|ui| {
            let deck_len = self.decks.get(self.active_deck_idx).map(|d| d.main.len()).unwrap_or(40);
            ui.label("Hand Probability Calculator:");
            ui.add(egui::DragValue::new(&mut self.prob_copies).range(1..=40).prefix("Copies: "));
            ui.add(egui::DragValue::new(&mut self.prob_hand).range(1..=10).prefix("Hand: "));
            ui.add(egui::DragValue::new(&mut self.prob_min).range(1..=5).prefix("At least: "));
            let p = hypergeometric_at_least(deck_len, self.prob_copies, self.prob_hand, self.prob_min) * 100.0;
            ui.colored_label(accent, format!("Chance: {p:.2}%"));
        });

        ui.separator();

        // Search & Filter Card Pool
        ui.horizontal(|ui| {
            ui.label("Search:");
            ui.text_edit_singleline(&mut self.filter_text);
            ui.selectable_value(&mut self.filter_type, 0, "All");
            ui.selectable_value(&mut self.filter_type, 1, "Monster");
            ui.selectable_value(&mut self.filter_type, 2, "Spell");
            ui.selectable_value(&mut self.filter_type, 3, "Trap");
            ui.selectable_value(&mut self.filter_type, 4, "Extra");
        });

        let filter = DeckFilter {
            text: if self.filter_text.is_empty() { None } else { Some(self.filter_text.clone()) },
            card_type_any: match self.filter_type {
                1 => Some(CardType::MONSTER),
                2 => Some(CardType::SPELL),
                3 => Some(CardType::TRAP),
                4 => Some(CardType::EXTRA_DECK),
                _ => None,
            },
            sort: SortBy::NameAsc,
            ..Default::default()
        };
        let results = filter.search(&self.db, Some(&bl));
        ui.label(format!(
            "Card Catalog: {} total cards | Showing {} of {} matches",
            self.db.len(),
            results.len().min(200),
            results.len()
        ));
        egui::ScrollArea::vertical().show(ui, |ui| {
            for c in results.into_iter().take(200) {
                ui.horizontal(|ui| {
                    let lim = bl.max_copies(c.effective_alias());
                    ui.label(format!("[{lim}x] {} ({}) ATK:{}/DEF:{}", c.name, c.code, c.atk, c.def));
                    if ui.button("+Add").clicked() {
                        if let Some(d) = self.decks.get_mut(self.active_deck_idx) {
                            if c.is_extra_deck() {
                                d.extra.push(c.code);
                            } else {
                                d.main.push(c.code);
                            }
                        }
                    }
                });
            }
        });
    }

    fn ui_puzzles(&mut self, ui: &mut egui::Ui, accent: egui::Color32) {
        ui.colored_label(accent, egui::RichText::new("Duel Puzzles (Lua Debug.* Engine)").strong().size(16.0));
        ui.horizontal(|ui| {
            for p in &self.puzzles {
                if ui.button(format!("{} [{}]", p.title, p.difficulty)).clicked() {
                    if let Ok(sess) = PuzzleSession::start(p.clone(), &self.db) {
                        self.active_puzzle = Some(sess);
                    }
                }
            }
        });
        if let Some(ref sess) = self.active_puzzle {
            ui.separator();
            ui.label(format!("Objective: {}", sess.puzzle.objective));
            for h in sess.hints() {
                ui.colored_label(egui::Color32::LIGHT_BLUE, format!("Hint: {h}"));
            }
            ui.label(format!(
                "Your LP: {} | AI LP: {} | Solved: {}",
                sess.engine.get_lp(0),
                sess.engine.get_lp(1),
                sess.is_solved()
            ));
        }
    }

    fn ui_hand_test(&mut self, ui: &mut egui::Ui, accent: egui::Color32) {
        ui.colored_label(accent, egui::RichText::new("Solo Hand Test & Combo Sandbox").strong().size(16.0));
        ui.horizontal(|ui| {
            if ui.button("Draw New 5-Card Hand").clicked() {
                self.sandbox_seed = self.sandbox_seed.wrapping_add(17);
                let deck = self.decks.get(self.active_deck_idx).cloned().unwrap_or_default();
                self.sandbox = Some(HandTestSandbox::new(deck, self.selected_format, self.sandbox_seed, &self.db));
            }
            if ui.button("Draw +1 Card").clicked() {
                if let Some(ref mut sb) = self.sandbox {
                    sb.draw_one();
                }
            }
        });
        if let Some(ref sb) = self.sandbox {
            ui.separator();
            for (i, name) in sb.hand_names().iter().enumerate() {
                ui.label(format!("{}. {name}", i + 1));
            }
        }
    }

    fn ui_tournaments_ranked(&mut self, ui: &mut egui::Ui, accent: egui::Color32) {
        ui.columns(2, |cols| {
            let ui = &mut cols[0];
            ui.colored_label(accent, egui::RichText::new("Seasonal Elo Leaderboard").strong().size(16.0));
            for (rank, row) in self.ranked.leaderboard(FormatPreset::TcgAdvanced, 10).iter().enumerate() {
                ui.label(format!(
                    "#{}: {} — {} Elo ({:?}) | {}W-{}L",
                    rank + 1,
                    row.player,
                    row.elo,
                    row.tier(),
                    row.wins,
                    row.losses
                ));
            }

            let ui = &mut cols[1];
            ui.colored_label(accent, egui::RichText::new(format!("Tournament: {}", self.tournament.name)).strong().size(16.0));
            ui.label(format!("Round {} | Standings:", self.tournament.current_round));
            for s in self.tournament.standings() {
                ui.label(format!(
                    "#{}: {} — {} pts ({}W-{}L-{}D, OMW {:.1}%)",
                    s.rank, s.player, s.match_points, s.wins, s.losses, s.draws, s.omw_percent * 100.0
                ));
            }
            ui.separator();
            ui.label(format!(
                "Draft Session: Round {}/{} | Pool size: {}",
                self.draft.current_round,
                self.draft.total_rounds,
                self.draft.pools[0].len()
            ));
        });
    }

    fn ui_replays(&mut self, ui: &mut egui::Ui, accent: egui::Color32) {
        ui.colored_label(accent, egui::RichText::new("Deterministic Replay Viewer (.yrp / .yrpx / .omegarp)").strong().size(16.0));
        if let Some(ref mut rp) = self.replay_player {
            ui.horizontal(|ui| {
                if ui.button("<< Start").clicked() { rp.seek(0); }
                if ui.button("< Step Back").clicked() { rp.step_back(); }
                if ui.button("Step Forward >").clicked() { rp.step_forward(); }
                if ui.button("End >>").clicked() { rp.seek(rp.total_frames().saturating_sub(1)); }
                ui.label(format!("Frame {} / {}", rp.cursor + 1, rp.total_frames()));
            });
            let st = rp.current_state();
            ui.label(format!(
                "Turn {} | Phase {:?} | P1 LP: {} | P2 LP: {}",
                st.turn_count, st.phase, st.players[0].lp, st.players[1].lp
            ));
        } else {
            ui.label("Play a duel and click 'Save Replay' in the Duel Arena to watch & scrub frames here.");
        }
    }

    fn ui_customize(&mut self, ui: &mut egui::Ui, accent: egui::Color32) {
        ui.colored_label(accent, egui::RichText::new("Duelist Customization (Sleeves, Playmats, Audio, Chain)").strong().size(16.0));
        ui.horizontal(|ui| {
            ui.label("Duelist Name:");
            ui.text_edit_singleline(&mut self.profile.duelist_name);
        });
        ui.horizontal(|ui| {
            ui.label("Playmat Theme:");
            for t in [
                PlaymatTheme::CyberNeon,
                PlaymatTheme::DuelistKingdom,
                PlaymatTheme::BattleCity,
                PlaymatTheme::VrainsLink,
                PlaymatTheme::ShadowRealm,
            ] {
                ui.selectable_value(&mut self.profile.playmat_theme, t, format!("{t:?}"));
            }
        });
        ui.horizontal(|ui| {
            ui.label("Card Sleeves:");
            for s in [
                SleeveStyle::MillenniumGold,
                SleeveStyle::DarkMagicianPurple,
                SleeveStyle::BlueEyesSilver,
                SleeveStyle::CyberCircuit,
                SleeveStyle::ClassicBack,
            ] {
                ui.selectable_value(&mut self.profile.card_sleeve, s, format!("{s:?}"));
            }
        });
        ui.add(egui::Slider::new(&mut self.profile.bgm_volume, 0..=100).text("BGM Volume"));
        ui.add(egui::Slider::new(&mut self.profile.sfx_volume, 0..=100).text("SFX Volume"));
    }
}

/// Launch the native pure-Rust desktop window!
pub fn launch_gui() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_title("Neon YGO Sim — 100% Pure-Rust Yu-Gi-Oh! Simulator"),
        ..Default::default()
    };
    eframe::run_native(
        "Neon YGO Sim",
        options,
        Box::new(|_cc| Ok(Box::new(NeonYgoApp::new()))),
    )
}
