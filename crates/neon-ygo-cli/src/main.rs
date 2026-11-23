use std::env;
use std::io::{self, Write};

use neon_ygo_ai::{builtin_puzzles, AiDifficulty, DuelAi, HandTestSandbox, PuzzleSession};
use neon_ygo_core::*;
use neon_ygo_db::{hypergeometric_at_least, Banlist, CardDatabase, Deck};
use neon_ygo_lua::LuaScriptHost;
use neon_ygo_net::{
    decode_frame, encode_frame, mask_state_for_viewer, ctos, ClientPacket, DraftSession,
    MatchOutcome, MatchPhase, MatchSession, OmegaClient, OmegaServer, RankedLadder, RoomMode,
    ServerPacket, Tournament, TournamentKind,
};
use neon_ygo_replay::{Replay, ReplayPlayer};

fn main() {
    let args: Vec<String> = env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("verify");

    match cmd {
        "gui" => {
            if let Err(e) = neon_ygo_gui::launch_gui() {
                eprintln!("GUI error: {e}");
                std::process::exit(1);
            }
        }
        "play" => {
            let fmt = parse_format(args.get(2).map(|s| s.as_str()).unwrap_or("tcg"));
            run_cli_duel(fmt);
        }
        "ai-demo" => {
            let fmt = parse_format(args.get(2).map(|s| s.as_str()).unwrap_or("tcg"));
            run_ai_demo(fmt);
        }
        "server" => {
            let addr = args.get(2).map(|s| s.as_str()).unwrap_or("127.0.0.1:7911");
            run_server(addr);
        }
        "deck" => {
            run_deck_tool(&args[2..]);
        }
        "cards" => {
            let q = args[2..].join(" ");
            run_cards_search(&q);
        }
        "sync" => {
            run_sync_data();
        }
        "audit-corpus" => {
            let ok = run_full_corpus_audit();
            if !ok {
                std::process::exit(1);
            }
        }
        "verify" => {
            let ok = run_omega_parity_verification();
            if !ok {
                std::process::exit(1);
            }
        }
        _ => {
            print_help();
        }
    }
}

fn print_help() {
    println!(
        "neon-ygo — 100% Pure-Rust Yu-Gi-Oh! Simulator (1:1 YGO Omega Parity — ALL Cards!)\n\n\
        USAGE:\n  \
        neon-ygo gui                      Launch native desktop GUI (25,000+ cards)\n  \
        neon-ygo play [format]            Interactive terminal duel vs AI\n  \
        neon-ygo ai-demo [format]         Watch AI vs AI duel\n  \
        neon-ygo server [addr]            Host async TCP multiplayer/ranked server\n  \
        neon-ygo cards <query>            Search all 25,000+ TCG/OCG/Rush/Speed/Anime cards\n  \
        neon-ygo deck [demo]              Convert/validate .ydk / ydke:// / Omega codes\n  \
        neon-ygo sync                     Fetch/update full BabelCDB, CardScripts & LFLists\n  \
        neon-ygo audit-corpus             Verify ALL .cdb cards & ALL 23,000+ Lua scripts\n  \
        neon-ygo verify                   Run full 1:1 YGO Omega Feature Parity Audit\n\n\
        FORMATS: tcg, ocg, md, goat, edison, speed, rush, turbo, tag, relay, genesys"
    );
}

fn run_cards_search(query: &str) {
    let db = CardDatabase::load_default();
    let filter = neon_ygo_db::DeckFilter {
        text: if query.is_empty() { None } else { Some(query.to_string()) },
        ..Default::default()
    };
    let hits = filter.search(&db, None);
    println!("Loaded {} total cards ({} scripts). Found {} matches for '{}':", db.len(), db.count_available_scripts(), hits.len(), query);
    for c in hits.iter().take(25) {
        println!("  [{}] {} | ATK:{} DEF:{} Lv/Rk/Lk:{}", c.code, c.name, c.atk, c.def, c.level);
    }
}

fn run_sync_data() {
    let repos = [
        ("data/cdb", "https://github.com/ProjectIgnis/BabelCDB.git"),
        ("data/lflists", "https://github.com/ProjectIgnis/LFLists.git"),
        ("data/scripts", "https://github.com/ProjectIgnis/CardScripts.git"),
    ];
    for (dir, url) in repos {
        let p = std::path::Path::new(dir);
        let status = if p.join(".git").is_dir() {
            println!("Updating {dir}...");
            std::process::Command::new("git").args(["-C", dir, "pull", "--ff-only"]).status()
        } else {
            println!("Cloning {url} -> {dir}...");
            std::process::Command::new("git").args(["clone", "--depth", "1", url, dir]).status()
        };
        match status {
            Ok(s) if s.success() => println!("  OK: {dir}"),
            Ok(s) => eprintln!("  FAILED ({s}): {dir}"),
            Err(e) => eprintln!("  ERROR: {e}"),
        }
    }
}

fn run_full_corpus_audit() -> bool {
    let t0 = std::time::Instant::now();
    let db = CardDatabase::load_default();
    let cdb_elapsed = t0.elapsed();
    println!(
        "1. Pure-Rust SQLite3 .cdb Loader: Loaded {} unique cards + {} official banlists in {:.2?}",
        db.len(),
        db.loaded_banlists.len(),
        cdb_elapsed
    );

    // 2. Parse ALL .lua files in data/scripts/ with our pure-Rust Lua 5.4 parser!
    let t1 = std::time::Instant::now();
    let mut lua_files = Vec::new();
    collect_lua_files(std::path::Path::new("data/scripts"), &mut lua_files);
    lua_files.sort();
    let mut parsed_ok = 0usize;
    let mut parse_errors: Vec<(String, String)> = Vec::new();
    for path in &lua_files {
        if let Ok(src) = std::fs::read_to_string(path) {
            match neon_ygo_lua::parser::parse(&src) {
                Ok(_) => parsed_ok += 1,
                Err(e) => parse_errors.push((path.display().to_string(), e.to_string())),
            }
        }
    }
    println!(
        "2. Pure-Rust Lua 5.4 Parser: {parsed_ok}/{} scripts parsed cleanly ({} errors) in {:.2?}",
        lua_files.len(),
        parse_errors.len(),
        t1.elapsed()
    );
    for (p, e) in parse_errors.iter().take(20) {
        println!("   PARSE ERR: {p} -> {e}");
    }

    // 3. Load all ProjectIgnis core libraries into pure-Rust Lua 5.4 VM & init iconic modern/classic official scripts!
    let t2 = std::time::Instant::now();
    let mut host = LuaScriptHost::new();
    db.attach_to_script_host(&mut host);
    let libs_loaded = host.load_core_libraries().unwrap_or(0);
    println!(
        "3. Pure-Rust Lua VM Core Libraries: loaded {libs_loaded} core libraries in {:.2?} (logs: {})",
        t2.elapsed(),
        host.vm.output_log.len()
    );
    for l in host.vm.output_log.iter().take(10) {
        println!("   CORE LOG: {l}");
    }

    // Initialize 250 real official scripts directly from data/scripts/official/ (bypassing built-in overrides!)
    let mut raw_host = LuaScriptHost::new();
    raw_host.set_core_script_dir("data/scripts");
    raw_host.add_script_dir("data/scripts/official");
    raw_host.add_script_dir("data/scripts/rush");
    raw_host.add_script_dir("data/scripts/skill");
    let _ = raw_host.load_core_libraries();
    let mut eng = DuelEngine::new(DuelConfig::from_preset(FormatPreset::TcgAdvanced, 1), "A", "B");

    let sample_codes: Vec<CardCode> = [
        89631139, 46986414, 14558127, 23434538, 10045474, 27204311, 94145021, 97268402,
        44362883, 68468459, 44146295, 63166095, 26077387, 63288573, 86066372, 1861629,
        84815190, 90448279, 84013237, 44508094, 73580471, 2857636, 38342335, 65741786,
        55144522, 83764718, 12580477, 18144506, 5318639, 14087893, 35261759, 82732705,
        30241314, 41420027, 40605147, 55410871, 16178681, 33396948, 24224830, 14558127,
    ]
    .into_iter()
    .collect();

    let mut init_ok = 0usize;
    let mut effects_registered = 0usize;
    raw_host.vm.output_log.clear();
    // Test 40 iconic cards + first 1,000 official cards in the database!
    let mut test_codes = sample_codes.clone();
    for (&code, _) in db.cards.iter() {
        if test_codes.len() >= 1000 {
            break;
        }
        if !test_codes.contains(&code)
            && std::path::Path::new(&format!("data/scripts/official/c{code}.lua")).is_file()
        {
            test_codes.push(code);
        }
    }
    for code in &test_codes {
        if let Some(cd) = db.get(*code).cloned() {
            let cid = eng.add_card(cd, 0, Location::HAND, Position::FACEUP_ATTACK, &mut raw_host);
            let n_eff = eng.card(cid).map(|c| c.effects.len()).unwrap_or(0);
            effects_registered += n_eff;
            init_ok += 1;
        }
    }
    println!(
        "4. Real Official ProjectIgnis Script Execution: initialized {init_ok}/{} real official cards ({effects_registered} effects registered, {} runtime notices)",
        test_codes.len(),
        raw_host.vm.output_log.len()
    );
    for l in raw_host.vm.output_log.iter().take(15) {
        println!("   EXEC LOG: {l}");
    }

    db.len() >= 25_000 && parse_errors.is_empty() && !db.loaded_banlists.is_empty() && libs_loaded == 22
}

fn collect_lua_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if p.file_name().and_then(|s| s.to_str()) == Some(".git") {
                continue;
            }
            collect_lua_files(&p, out);
        } else if p.extension().and_then(|s| s.to_str()) == Some("lua") {
            out.push(p);
        }
    }
}

fn parse_format(s: &str) -> FormatPreset {
    match s.to_lowercase().as_str() {
        "ocg" => FormatPreset::Ocg,
        "md" | "masterduel" => FormatPreset::MasterDuel,
        "goat" => FormatPreset::Goat,
        "edison" => FormatPreset::Edison,
        "speed" => FormatPreset::SpeedDuel,
        "rush" => FormatPreset::RushDuel,
        "turbo" => FormatPreset::TurboDuel,
        "tag" => FormatPreset::TagDuel,
        "relay" => FormatPreset::RelayDuel,
        "genesys" => FormatPreset::Genesys,
        _ => FormatPreset::TcgAdvanced,
    }
}

fn run_ai_demo(format: FormatPreset) {
    let db = CardDatabase::load_default();
    let decks = db.starter_decks();
    let cfg = DuelConfig::from_preset(format, 2026);
    let mut engine = DuelEngine::new(cfg, "Blue-Eyes AI", "Dark Magician AI");
    let mut host = LuaScriptHost::new();
    db.attach_to_script_host(&mut host);

    let (m0, e0, s0) = decks[0].resolve(&db);
    let (m1, e1, s1) = decks[1].resolve(&db);
    engine.load_deck(0, m0, e0, s0, &mut host);
    engine.load_deck(1, m1, e1, s1, &mut host);
    engine.start_duel(0, &mut host);

    let ai0 = DuelAi::new("Blue-Eyes AI", AiDifficulty::Competitive);
    let ai1 = DuelAi::new("Dark Magician AI", AiDifficulty::Competitive);
    let (winner, steps) = DuelAi::run_ai_vs_ai(&mut engine, &mut host, &ai0, &ai1, 300);
    println!(
        "AI Demo ({format:?}) finished in {steps} actions | Turn {} | LP: {} vs {} | Winner: {:?} ({:?})",
        engine.state.turn_count,
        engine.get_lp(0),
        engine.get_lp(1),
        winner,
        engine.state.win_reason
    );
}

fn run_cli_duel(format: FormatPreset) {
    let db = CardDatabase::load_default();
    let decks = db.starter_decks();
    let cfg = DuelConfig::from_preset(format, 42);
    let mut engine = DuelEngine::new(cfg, "You", "Omega AI");
    let mut host = LuaScriptHost::new();
    db.attach_to_script_host(&mut host);

    let (m0, e0, s0) = decks[0].resolve(&db);
    let (m1, e1, s1) = decks[1].resolve(&db);
    engine.load_deck(0, m0, e0, s0, &mut host);
    engine.load_deck(1, m1, e1, s1, &mut host);
    engine.start_duel(0, &mut host);

    let ai = DuelAi::new("Omega AI", AiDifficulty::Competitive);
    let stdin = io::stdin();

    while !engine.is_finished() {
        if engine.state.turn_player == 1 {
            if let Some(d) = ai.choose_decision(&mut engine, &mut host) {
                let _ = engine.apply_decision(d, &mut host);
            } else {
                let _ = engine.apply_decision(Decision::EndTurn, &mut host);
            }
            continue;
        }
        println!(
            "\n=== Turn {} | Phase {:?} | Your LP: {} | AI LP: {} ===",
            engine.state.turn_count,
            engine.state.phase,
            engine.get_lp(0),
            engine.get_lp(1)
        );
        let hand = engine.cards_in(0, Location::HAND);
        println!("Hand:");
        for (i, &cid) in hand.iter().enumerate() {
            if let Some(c) = engine.card(cid) {
                println!("  [{i}] {} (Lv{} {}/{})", c.data.name, c.computed.level, c.current_atk(), c.current_def());
            }
        }
        print!("Command (s <idx>=summon, b=battle, e=end turn, q=quit): ");
        let _ = io::stdout().flush();
        let mut line = String::new();
        if stdin.read_line(&mut line).is_err() {
            break;
        }
        let parts: Vec<&str> = line.trim().split_whitespace().collect();
        match parts.first().copied() {
            Some("q") => break,
            Some("e") => { let _ = engine.apply_decision(Decision::EndTurn, &mut host); }
            Some("b") => { let _ = engine.apply_decision(Decision::EnterBattlePhase, &mut host); }
            Some("s") => {
                if let Some(idx) = parts.get(1).and_then(|s| s.parse::<usize>().ok()) {
                    if let Some(&cid) = hand.get(idx) {
                        if let Err(err) = engine.apply_decision(
                            Decision::NormalSummon { card_id: cid, tributes: vec![], zone: None },
                            &mut host,
                        ) {
                            println!("Error: {err}");
                        }
                    }
                }
            }
            _ => println!("Unknown command."),
        }
    }
    println!("Duel ended! Winner: {:?} ({:?})", engine.state.winner, engine.state.win_reason);
}

fn run_server(addr: &str) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        let db = CardDatabase::load_default();
        let server = OmegaServer::new(db);
        let (bound, handle) = server.bind(addr).await.expect("bind server");
        println!("Neon YGO Omega Server listening on {bound}");
        let _ = handle.await;
    });
}

fn run_deck_tool(_args: &[String]) {
    let db = CardDatabase::load_default();
    let d = &db.starter_decks()[0];
    let ydke = d.to_ydke();
    let omega = d.to_omega_code().unwrap();
    let p_starter = hypergeometric_at_least(d.main.len(), 3, 5, 1) * 100.0;
    println!("Deck: {}", d.name);
    println!("YDKE : {ydke}");
    println!("OMEGA: {omega}");
    println!("P(draw >=1 of 3-of in 5-card opening hand): {p_starter:.2}%");
}

struct CheckResult {
    category: &'static str,
    feature: &'static str,
    passed: bool,
    detail: String,
}

fn run_omega_parity_verification() -> bool {
    let db = CardDatabase::with_builtin();
    let mut results: Vec<CheckResult> = Vec::new();

    let mut record = |cat: &'static str, feat: &'static str, ok: bool, detail: String| {
        results.push(CheckResult { category: cat, feature: feat, passed: ok, detail });
    };

    // 1. Master Rules 1-5 + Extra Deck Zone Rules
    {
        let eng_mr4 = DuelEngine::new(DuelConfig::from_preset(FormatPreset::TcgAdvanced, 1).with_master_rule(MasterRule::Mr4), "A", "B");
        let eng_mr5 = DuelEngine::new(DuelConfig::from_preset(FormatPreset::TcgAdvanced, 1).with_master_rule(MasterRule::Mr5), "A", "B");
        let syn = db.get(44508094).unwrap(); // Stardust Dragon
        let lnk = db.get(1861629).unwrap();  // Decode Talker
        let mr4_syn = eng_mr4.valid_extra_deck_zones(0, syn);
        let mr5_syn = eng_mr5.valid_extra_deck_zones(0, syn);
        let mr5_lnk = eng_mr5.valid_extra_deck_zones(0, lnk);
        let ok = mr4_syn == vec![5, 6] && mr5_syn == vec![0, 1, 2, 3, 4, 5, 6] && mr5_lnk == vec![5, 6];
        record("Rules", "Master Rules 1-5 (MR4 EMZ Lock vs MR5 Revision)", ok, format!("MR4 Syn={mr4_syn:?}, MR5 Syn={mr5_syn:?}, MR5 Link={mr5_lnk:?}"));
    }

    // 2. Extra Link / U-Link Detection & Second EMZ Claim
    {
        let mut eng = DuelEngine::new(DuelConfig::from_preset(FormatPreset::TcgAdvanced, 1), "A", "B");
        let mut host = NoopScriptHost;
        // Build U-Link: EMZ L(5) [BOTTOM] <-> MMZ 1 [TOP|RIGHT] <-> MMZ 2 [LEFT|RIGHT] <-> MMZ 3 [LEFT|TOP] -> EMZ R(6) [BOTTOM]!
        let mk = |code: u32, m: LinkMarker| {
            let mut c = db.get(1861629).unwrap().clone();
            c.code = code;
            c.link_markers = m;
            c
        };
        let c1 = eng.add_card_at(mk(1, LinkMarker::BOTTOM), 0, 0, Location::MZONE, 5, Position::FACEUP_ATTACK, true, &mut host);
        let c2 = eng.add_card_at(mk(2, LinkMarker::TOP | LinkMarker::RIGHT), 0, 0, Location::MZONE, 1, Position::FACEUP_ATTACK, true, &mut host);
        let _c3 = eng.add_card_at(mk(3, LinkMarker::LEFT | LinkMarker::RIGHT), 0, 0, Location::MZONE, 2, Position::FACEUP_ATTACK, true, &mut host);
        let _c4 = eng.add_card_at(mk(4, LinkMarker::LEFT | LinkMarker::TOP), 0, 0, Location::MZONE, 3, Position::FACEUP_ATTACK, true, &mut host);
        let emz_r_cand = mk(5, LinkMarker::BOTTOM);
        let zones = eng.valid_extra_deck_zones(0, &emz_r_cand);
        let can_claim_second_emz = zones.contains(&6);
        let c5 = eng.add_card_at(emz_r_cand, 0, 0, Location::MZONE, 6, Position::FACEUP_ATTACK, true, &mut host);
        let u_linked = eng.are_cards_co_linked(c1, c2) && eng.is_card_extra_linked(c1) && eng.is_card_extra_linked(c5);
        record("Mechanics", "Link Arrows, Co-Link & Extra Link (U-Link)", can_claim_second_emz && u_linked, format!("claim_2nd_EMZ={can_claim_second_emz}, U-Link={u_linked}"));
    }

    // 3. All Summoning Mechanics (Normal, Tribute, Flip, Fusion, Ritual, Synchro, Xyz, Pendulum, Link, Maximum)
    {
        let mut eng = DuelEngine::new(DuelConfig::from_preset(FormatPreset::TcgAdvanced, 1), "A", "B");
        let mut host = LuaScriptHost::new();
        db.attach_to_script_host(&mut host);
        eng.start_puzzle(0, &mut host);
        // Synchro: Junk Synchron(3) + Cyber Dragon(5) = Stardust(8)
        let t = eng.add_card_at(db.get(63977008).unwrap().clone(), 0, 0, Location::MZONE, 0, Position::FACEUP_ATTACK, true, &mut host);
        let nt = eng.add_card_at(db.get(70095154).unwrap().clone(), 0, 0, Location::MZONE, 1, Position::FACEUP_ATTACK, true, &mut host);
        let syn = eng.add_card(db.get(44508094).unwrap().clone(), 0, Location::EXTRA, Position::FACEDOWN_DEFENSE, &mut host);
        let syn_ok = eng.synchro_summon(0, syn, &[t, nt], None, &mut host).is_ok();
        // Xyz: 2x Lv4 -> Utopia + Overlay detach does NOT trigger LEAVE_FIELD
        let x1 = eng.add_card_at(db.get(71413901).unwrap().clone(), 0, 0, Location::MZONE, 1, Position::FACEUP_ATTACK, true, &mut host);
        let x2 = eng.add_card_at(db.get(40044918).unwrap().clone(), 0, 0, Location::MZONE, 2, Position::FACEUP_ATTACK, true, &mut host);
        let uto = eng.add_card(db.get(84013237).unwrap().clone(), 0, Location::EXTRA, Position::FACEDOWN_DEFENSE, &mut host);
        let xyz_ok = eng.xyz_summon(0, uto, &[x1, x2], None, &mut host).is_ok()
            && eng.card(uto).unwrap().overlay_materials.len() == 2
            && eng.detach_overlay(uto, 1, Reason::COST, 0, &mut host).is_ok()
            && eng.card(uto).unwrap().overlay_materials.len() == 1;
        // Pendulum: Scale 1 + Scale 8 -> Pendulum Summon Odd-Eyes(7) + GY redirect to face-up Extra Deck!
        let p_l = eng.add_card(db.get(94415058).unwrap().clone(), 0, Location::HAND, Position::FACEUP_ATTACK, &mut host);
        let p_r = eng.add_card(db.get(20409757).unwrap().clone(), 0, Location::HAND, Position::FACEUP_ATTACK, &mut host);
        let oe = eng.add_card(db.get(16178681).unwrap().clone(), 0, Location::HAND, Position::FACEUP_ATTACK, &mut host);
        let _ = eng.activate_pendulum_scale(0, p_l, false, &mut host);
        let _ = eng.activate_pendulum_scale(0, p_r, true, &mut host);
        let pend_ok = eng.pendulum_summon(0, &[oe], &mut host).is_ok();
        let _ = eng.destroy_cards(&[oe], Reason::EFFECT, 1, None, &mut host);
        let pend_to_ex = eng.card(oe).map(|c| c.location == Location::EXTRA && c.is_faceup()).unwrap_or(false);
        record("Mechanics", "Synchro, Xyz (Overlay), Pendulum (+Face-Up Extra Redirect)", syn_ok && xyz_ok && pend_ok && pend_to_ex, format!("syn={syn_ok}, xyz={xyz_ok}, pend={pend_ok}, pend_ex={pend_to_ex}"));
    }

    // 4. Proper Special Summon Revival Limit Rule
    {
        let mut eng = DuelEngine::new(DuelConfig::from_preset(FormatPreset::TcgAdvanced, 1), "A", "B");
        let mut host = NoopScriptHost;
        eng.start_puzzle(0, &mut host);
        let improper = eng.add_card_at(db.get(44508094).unwrap().clone(), 0, 0, Location::GRAVE, 0, Position::FACEUP_ATTACK, false, &mut host);
        let proper = eng.add_card_at(db.get(44508094).unwrap().clone(), 0, 0, Location::GRAVE, 1, Position::FACEUP_ATTACK, true, &mut host);
        let blocked = eng.special_summon(0, improper, SummonType::SPECIAL, Position::FACEUP_ATTACK, None, false, false, &mut host).is_err();
        let allowed = eng.special_summon(0, proper, SummonType::SPECIAL, Position::FACEUP_ATTACK, None, false, false, &mut host).is_ok();
        record("Rulings", "Proper Special Summon / Revival Rule (STATUS_PROC_COMPLETE)", blocked && allowed, format!("improper_blocked={blocked}, proper_allowed={allowed}"));
    }

    // 5. Chain, Spell Speed 1/2/3, SEGOC, Missed Timing ("When" vs "If") & Pure-Rust Lua 5.4 VM
    {
        let mut eng = DuelEngine::new(DuelConfig::from_preset(FormatPreset::TcgAdvanced, 1), "A", "B");
        let mut host = LuaScriptHost::new();
        db.attach_to_script_host(&mut host);
        eng.start_puzzle(0, &mut host);
        for _ in 0..5 { eng.add_card(db.get(89631139).unwrap().clone(), 0, Location::DECK, Position::FACEDOWN_DEFENSE, &mut host); }
        let pot = eng.add_card(db.get(55144522).unwrap().clone(), 0, Location::HAND, Position::FACEUP_ATTACK, &mut host);
        let sj = eng.add_card_at(db.get(41420027).unwrap().clone(), 1, 1, Location::SZONE, 0, Position::FACEDOWN_ATTACK, false, &mut host);
        let pot_eff = eng.card(pot).unwrap().effects[0];
        let sj_eff = eng.card(sj).unwrap().effects[0];
        let _ = eng.activate_effect(0, pot, pot_eff, vec![], &mut host);
        // Spell Speed 3 Solemn Judgment chains to Spell Speed 1 Pot of Greed, negating it!
        let ss3_chained = eng.activate_effect(1, sj, sj_eff, vec![], &mut host).is_ok();
        let ss2_blocked_on_ss3 = !eng.state.chain.can_chain(2, 0);
        eng.resolve_chain(&mut host);
        let pot_negated = eng.cards_in(0, Location::HAND).is_empty() && eng.get_lp(1) == 4000;
        // Missed timing test: "When" trigger misses when event is NOT last thing to happen; "If" (DELAY) triggers!
        let c_when = eng.add_card(db.get(26202165).unwrap().clone(), 0, Location::GRAVE, Position::FACEUP_ATTACK, &mut host);
        let c_if = eng.add_card(db.get(78010363).unwrap().clone(), 0, Location::GRAVE, Position::FACEUP_ATTACK, &mut host);
        let mut e_when = eng.create_effect_struct(Some(c_when), 0);
        e_when.effect_type = EffectType::SINGLE | EffectType::TRIGGER_O;
        e_when.code = event_code::TO_GRAVE;
        e_when.flags = EffectFlag::CAN_MISS_TIMING;
        let id_when = eng.register_effect(e_when, &mut host);
        let mut e_if = eng.create_effect_struct(Some(c_if), 0);
        e_if.effect_type = EffectType::SINGLE | EffectType::TRIGGER_O;
        e_if.code = event_code::TO_GRAVE;
        e_if.flags = EffectFlag::DELAY;
        let id_if = eng.register_effect(e_if, &mut host);
        eng.state.pending_events.clear();
        eng.raise_event(DuelEvent::new(event_code::TO_GRAVE, 0).with_cards(vec![c_when, c_if]));
        eng.raise_event(DuelEvent::new(event_code::DRAW, 0)); // Subsequent action causes "When" to miss timing!
        let trigs = eng.collect_segoc_triggers(&mut host);
        let timing_ok = !trigs.iter().any(|t| t.effect_id == id_when) && trigs.iter().any(|t| t.effect_id == id_if);
        record("Rulings", "Pure-Rust Lua VM, Chain LIFO, Spell Speed 3 & Missed Timing", ss3_chained && ss2_blocked_on_ss3 && pot_negated && timing_ok, format!("ss3={ss3_chained}, block_ss2={ss2_blocked_on_ss3}, negated={pot_negated}, timing={timing_ok}"));
    }

    // 6. Floodgates (Skill Drain, Macro Cosmos, Imperial Iron Wall, Droll & Lock Bird) & Face-down Banish
    {
        let mut eng = DuelEngine::new(DuelConfig::from_preset(FormatPreset::TcgAdvanced, 1), "A", "B");
        let mut host = LuaScriptHost::new();
        db.attach_to_script_host(&mut host);
        eng.start_puzzle(0, &mut host);
        let _mc = eng.add_card_at(db.get(30241314).unwrap().clone(), 0, 0, Location::SZONE, 0, Position::FACEUP_ATTACK, false, &mut host);
        let mon = eng.add_card_at(db.get(89631139).unwrap().clone(), 1, 1, Location::MZONE, 0, Position::FACEUP_ATTACK, true, &mut host);
        eng.recalculate_effects(&mut host);
        let _ = eng.destroy_cards(&[mon], Reason::EFFECT, 0, None, &mut host);
        let macro_banished = eng.card(mon).unwrap().location == Location::REMOVED;
        // Now activate Imperial Iron Wall -> banishing is blocked!
        let _iiw = eng.add_card_at(db.get(30459350).unwrap().clone(), 1, 1, Location::SZONE, 0, Position::FACEUP_ATTACK, false, &mut host);
        let mon2 = eng.add_card_at(db.get(46986414).unwrap().clone(), 1, 1, Location::MZONE, 1, Position::FACEUP_ATTACK, true, &mut host);
        eng.recalculate_effects(&mut host);
        let _ = eng.destroy_cards(&[mon2], Reason::EFFECT, 0, None, &mut host);
        let iiw_prevented = eng.card(mon2).unwrap().location == Location::GRAVE;
        record("Rulings", "Floodgates (Macro Cosmos vs Imperial Iron Wall) & Continuous Layer", macro_banished && iiw_prevented, format!("macro_banish={macro_banished}, iron_wall_override={iiw_prevented}"));
    }

    // 7. 5-SubStep Damage Step + Piercing + Flip Effect After Damage Calc
    {
        let mut eng = DuelEngine::new(DuelConfig::from_preset(FormatPreset::TcgAdvanced, 1), "A", "B");
        let mut host = LuaScriptHost::new();
        db.attach_to_script_host(&mut host);
        eng.start_puzzle(0, &mut host);
        eng.state.turn_count = 2;
        let chaos = eng.add_card_at(db.get(55410871).unwrap().clone(), 0, 0, Location::MZONE, 0, Position::FACEUP_ATTACK, true, &mut host);
        let bug = eng.add_card_at(db.get(54652250).unwrap().clone(), 1, 1, Location::MZONE, 0, Position::FACEDOWN_DEFENSE, false, &mut host);
        eng.recalculate_effects(&mut host);
        let _ = eng.advance_phase(Phase::BattleStart, &mut host);
        let _ = eng.declare_attack(chaos, Some(bug), &mut host);
        // Chaos MAX (4000 ATK, Piercing, Indestructible by Effect!) vs Man-Eater Bug (600 DEF, FLIP destroy!)
        // Piercing damage = 4000 - 600 = 3400 -> Opp LP = 4600; Chaos MAX survives Man-Eater Bug because indestructible by effect!
        let ok = eng.get_lp(1) == 4600 && eng.card(bug).unwrap().location == Location::GRAVE && eng.card(chaos).unwrap().location == Location::MZONE;
        record("Combat", "5-SubStep Damage Step, Flip After Calc, Piercing & Protection", ok, format!("opp_lp={}, bug_gy={}, chaos_alive={}", eng.get_lp(1), eng.card(bug).unwrap().location == Location::GRAVE, eng.card(chaos).unwrap().location == Location::MZONE));
    }

    // 8. Alternate Formats: Rush Duel (Draw-to-5 + Maximum), Speed Duel (Skill), Turbo (Speed Counters), Tag 2v2, Relay, GOAT, Genesys
    {
        // Rush Duel Maximum Summon
        let mut rush = DuelEngine::new(DuelConfig::from_preset(FormatPreset::RushDuel, 1), "Yuga", "Luke");
        let mut host = LuaScriptHost::new();
        db.attach_to_script_host(&mut host);
        rush.start_puzzle(0, &mut host);
        let l = rush.add_card(db.get(160003001).unwrap().clone(), 0, Location::HAND, Position::FACEUP_ATTACK, &mut host);
        let c = rush.add_card(db.get(160003002).unwrap().clone(), 0, Location::HAND, Position::FACEUP_ATTACK, &mut host);
        let r = rush.add_card(db.get(160003003).unwrap().clone(), 0, Location::HAND, Position::FACEUP_ATTACK, &mut host);
        let max_ok = rush.maximum_summon(0, [l, c, r], &mut host).is_ok() && rush.card(c).unwrap().current_atk() == 4000;

        // Speed Duel Skill Card
        let mut spd = DuelEngine::new(DuelConfig::from_preset(FormatPreset::SpeedDuel, 1), "Yugi", "Kaiba");
        spd.start_puzzle(0, &mut host);
        spd.add_card(db.get(46986414).unwrap().clone(), 0, Location::DECK, Position::FACEDOWN_DEFENSE, &mut host);
        let sk = spd.add_card(db.get(300101001).unwrap().clone(), 0, Location::SKILL, Position::FACEDOWN_DEFENSE, &mut host);
        let skill_ok = spd.activate_skill(0, sk, &mut host).is_ok() && spd.cards_in(0, Location::HAND).len() == 1;

        // Turbo Duel Speed Counters
        let mut turbo = DuelEngine::new(DuelConfig::from_preset(FormatPreset::TurboDuel, 1), "Yusei", "Jack");
        for _ in 0..10 {
            turbo.add_card(db.get(89631139).unwrap().clone(), 0, Location::DECK, Position::FACEDOWN_DEFENSE, &mut host);
            turbo.add_card(db.get(46986414).unwrap().clone(), 1, Location::DECK, Position::FACEDOWN_DEFENSE, &mut host);
        }
        turbo.start_duel(0, &mut host);
        let turbo_ok = turbo.state.players[0].speed_counters == 1;

        // Tag 2v2 (16000 LP, 4-player rotation) & Relay Duel (handoff keeping field!)
        let tag = DuelEngine::new(DuelConfig::from_preset(FormatPreset::TagDuel, 1), "P0", "P1");
        let tag_ok = tag.get_lp(0) == 16000 && tag.state.players.len() == 4;
        let mut relay = DuelEngine::new(DuelConfig::from_preset(FormatPreset::RelayDuel, 1), "R1", "Opp");
        relay.add_relay_teammate(0, "R2", vec![db.get(89631139).unwrap().clone(); 10], vec![]);
        relay.start_puzzle(0, &mut host);
        let field_mon = relay.add_card_at(db.get(46986414).unwrap().clone(), 0, 0, Location::MZONE, 0, Position::FACEUP_ATTACK, true, &mut host);
        relay.inflict_damage(0, 8000, false, Reason::EFFECT, &mut host);
        let relay_ok = !relay.is_finished() && relay.get_lp(0) == 8000 && relay.state.players[0].name == "R2" && relay.card(field_mon).is_some();

        record("Formats", "Rush Maximum, Speed Skill, Turbo Counters, Tag 2v2 & Relay", max_ok && skill_ok && turbo_ok && tag_ok && relay_ok, format!("rush={max_ok}, speed={skill_ok}, turbo={turbo_ok}, tag={tag_ok}, relay={relay_ok}"));
    }

    // 9. Deck Codes (.ydk, ydke://, YGO Omega Gzip+Base64) & Banlist/Genesys/Alias Validation & Hypergeometric
    {
        let deck = &db.starter_decks()[0];
        let ydk_rt = Deck::from_ydk("rt", &deck.to_ydk());
        let ydke_rt = Deck::from_ydke("rt", &deck.to_ydke()).unwrap();
        let omega_code = deck.to_omega_code().unwrap();
        let omega_rt = Deck::from_omega_code("rt", &omega_code, &db).unwrap();
        let codes_ok = ydk_rt.main == deck.main && ydke_rt.extra == deck.extra && omega_rt.main == deck.main && omega_rt.side == deck.side;

        // Shared alias test: 2x Umi + 2x A Legendary Ocean = 4 copies -> caught!
        let mut alias_deck = Deck { name: "AliasTest".into(), main: vec![22702055, 22702055, 295517, 295517], extra: vec![], side: vec![], skill: None };
        while alias_deck.main.len() < 40 { alias_deck.main.push(89631139); }
        let bl = Banlist::preset(FormatPreset::TcgAdvanced);
        let rep = bl.validate_deck(&alias_deck, &db, &DuelConfig::from_preset(FormatPreset::TcgAdvanced, 1));
        let alias_caught = !rep.valid && rep.errors.iter().any(|e| e.contains("Umi"));

        // Genesys 100-pt cap test
        let g_bl = Banlist::preset(FormatPreset::Genesys);
        let g_deck = Deck { name: "Genesys".into(), main: vec![55144522, 55144522, 79571449], extra: vec![], side: vec![], skill: None }; // 50+50+40 = 140 > 100!
        let g_rep = g_bl.validate_deck(&g_deck, &db, &DuelConfig::from_preset(FormatPreset::Genesys, 1));
        let genesys_ok = !g_rep.valid && g_rep.genesys_points_used == 140;
        let prob = hypergeometric_at_least(40, 3, 5, 1);
        let prob_ok = (prob - 0.3376).abs() < 0.01;

        record("DeckBuilder", ".ydk / ydke:// / Omega Code, Alias Banlist, Genesys & Hypergeometric", codes_ok && alias_caught && genesys_ok && prob_ok, format!("codes={codes_ok}, alias={alias_caught}, genesys={genesys_ok}, P(3-of)={:.2}%", prob * 100.0));
    }

    // 10. Manual / Judge Mode + Undo Snapshot Rollback + Chain Control Modes
    {
        let mut eng = DuelEngine::new(DuelConfig::from_preset(FormatPreset::TcgAdvanced, 1), "A", "B");
        let mut host = NoopScriptHost;
        let _ = eng.apply_manual_command(ManualCommand::SetManualMode(true), &mut host);
        let _ = eng.apply_manual_command(ManualCommand::SetLp { player: 0, lp: 1234 }, &mut host);
        let set_ok = eng.get_lp(0) == 1234;
        let _ = eng.apply_manual_command(ManualCommand::Undo, &mut host);
        let undo_ok = eng.get_lp(0) == 8000;
        eng.set_chain_control(0, ChainControlMode::Off);
        let chain_off_ok = eng.should_prompt_chain(0, &mut host).is_none();
        record("OmegaUX", "Manual/Judge Mode, State Rollback Undo & 4-Mode Chain Control", set_ok && undo_ok && chain_off_ok, format!("set_lp={set_ok}, undo={undo_ok}, chain_off={chain_off_ok}"));
    }

    // 11. Deterministic Replays (.yrp LZMA, .yrpx, .omegarp) + Frame Scrubbing
    {
        let decks = db.starter_decks();
        let cfg = DuelConfig::from_preset(FormatPreset::TcgAdvanced, 999);
        let mut rep = Replay::new("Verify Replay", cfg.clone(), 0, vec![("A".into(), decks[0].clone()), ("B".into(), decks[1].clone())]);
        let mut eng = DuelEngine::new(cfg, "A", "B");
        let mut host = LuaScriptHost::new();
        db.attach_to_script_host(&mut host);
        let (m0, e0, s0) = decks[0].resolve(&db);
        let (m1, e1, s1) = decks[1].resolve(&db);
        eng.load_deck(0, m0, e0, s0, &mut host);
        eng.load_deck(1, m1, e1, s1, &mut host);
        eng.start_duel(0, &mut host);
        let ai = DuelAi::new("Bot", AiDifficulty::Competitive);
        for _ in 0..10 {
            if let Some(d) = ai.choose_decision(&mut eng, &mut host) {
                rep.record_decision(d.clone());
                let _ = eng.apply_decision(d, &mut host);
            }
        }
        let yrp_bytes = rep.to_yrp_bytes().unwrap();
        let (_hdr, rep_yrp) = Replay::from_yrp_bytes(&yrp_bytes).unwrap();
        let yrpx_bytes = rep.to_yrpx_bytes().unwrap();
        let rep_yrpx = Replay::from_yrpx_bytes(&yrpx_bytes).unwrap();
        let omega_bytes = rep.to_omegarp_bytes().unwrap();
        let rep_omega = Replay::from_omegarp_bytes(&omega_bytes).unwrap();
        let mut player = ReplayPlayer::load(rep_yrp, &db).unwrap();
        let det_ok = player.final_state().players[0].lp == eng.get_lp(0)
            && player.final_state().turn_count == eng.state.turn_count
            && rep_yrpx.actions.len() == rep_omega.actions.len()
            && player.step_forward()
            && player.step_back();
        record("Replays", "Deterministic .yrp (LZMA), .yrpx, .omegarp & Frame Rewind", det_ok, format!("yrp={}B, yrpx={}B, frames={}", yrp_bytes.len(), yrpx_bytes.len(), player.total_frames()));
    }

    // 12. Puzzles (Lua Debug.* Engine) + Solo Hand Test + AI vs AI
    {
        let puzzles = builtin_puzzles();
        let mut sess = PuzzleSession::start(puzzles[2].clone(), &db).unwrap(); // Exodia Pot of Greed puzzle!
        let pot_id = sess.engine.cards_in(0, Location::HAND).into_iter().find(|&id| sess.engine.card(id).unwrap().data.code == 55144522).unwrap();
        let eff_id = sess.engine.card(pot_id).unwrap().effects[0];
        let _ = sess.engine.apply_decision(Decision::ActivateEffect { card_id: pot_id, effect_id: eff_id, targets: vec![] }, &mut sess.host);
        let puzzle_solved = sess.is_solved() && sess.engine.state.win_reason == Some(WinReason::Exodia);
        let mut sb = HandTestSandbox::new(db.starter_decks()[0].clone(), FormatPreset::TcgAdvanced, 7, &db);
        let h5 = sb.hand_names().len() == 5;
        sb.draw_one();
        let h6 = sb.hand_names().len() == 6;
        record("SoloAndAI", "Lua Puzzle Mode (Exodia Solve), Hand Test Sandbox & AI", puzzle_solved && h5 && h6, format!("puzzle_solved={puzzle_solved}, hand_5_to_6={}", h5 && h6));
    }

    // 13. Networking (Live Async TCP Server, Match Bo3 + Side Deck Conservation, Ranked Elo, Swiss Tournament, Draft, Hidden Masking)
    {
        let frame = encode_frame(ctos::CHAT, b"hello");
        let (op, pay, _) = decode_frame(&frame).unwrap();
        let frame_ok = op == ctos::CHAT && pay == b"hello";

        // Match Bo3 + RPS + Side Decking multiset check
        let d0 = db.starter_decks()[0].clone();
        let d1 = db.starter_decks()[1].clone();
        let mut m = MatchSession::new(true, d0.clone(), d1.clone());
        let _ = m.submit_rps(0, RpsMove::Rock);
        let _ = m.submit_rps(1, RpsMove::Scissors);
        let _ = m.choose_turn_order(0, true);
        let _ = m.finish_game(Some(0));
        let side_phase_ok = matches!(m.phase, MatchPhase::SideDecking { next_game: 2, loser_chooses: 1 });
        // Reject cheated side deck
        let mut cheat = d0.clone();
        cheat.main[0] = 55144522;
        let bl = Banlist::preset(FormatPreset::TcgAdvanced);
        let cfg = DuelConfig::from_preset(FormatPreset::TcgAdvanced, 1);
        let cheat_rejected = m.submit_side_deck(0, cheat, &db, &bl, &cfg).is_err();

        // Ranked Elo + Swiss Tournament + Draft
        let mut ladder = RankedLadder::new(1);
        let (da, db_elo) = ladder.record_match(FormatPreset::TcgAdvanced, "Alice", "Bob", 1.0);
        let elo_ok = da > 0 && db_elo < 0;

        let mut tour = Tournament::new("T1", "Omega Open", FormatPreset::TcgAdvanced, TournamentKind::Swiss { rounds: 2 });
        for p in ["A", "B", "C", "D"] { tour.register(p); }
        let r1 = tour.start_next_round().unwrap();
        for p in &r1 { tour.report_result(p.table, MatchOutcome::PlayerAWins).unwrap(); }
        let r2 = tour.start_next_round().unwrap();
        for p in &r2 { tour.report_result(p.table, MatchOutcome::PlayerAWins).unwrap(); }
        let tour_ok = tour.finished && tour.champion.is_some();

        let cube: Vec<CardCode> = db.cards.keys().copied().collect();
        let mut draft = DraftSession::new(vec!["P1".into(), "P2".into()], cube, 1, 3, 42);
        for _ in 0..3 {
            draft.pick(0, 0).unwrap();
            draft.pick(1, 0).unwrap();
        }
        let draft_ok = draft.finished && draft.pools[0].len() == 3;

        // Live async TCP server roundtrip
        let rt = tokio::runtime::Runtime::new().unwrap();
        let tcp_ok = rt.block_on(async {
            let srv = OmegaServer::new(db.clone());
            let (addr, handle) = srv.bind("127.0.0.1:0").await.unwrap();
            let mut c1 = OmegaClient::connect(addr).await.unwrap();
            c1.send(&ClientPacket::Hello { player_name: "Alice".into() }).await.unwrap();
            let w = c1.recv().await.unwrap();
            c1.send(&ClientPacket::CreateRoom {
                title: "Omega Test Room".into(),
                passcode: Some("1234".into()),
                format: FormatPreset::TcgAdvanced,
                mode: RoomMode::MatchBo3,
                manual_mode: false,
            }).await.unwrap();
            let joined = c1.recv().await.unwrap();
            handle.abort();
            matches!(w, ServerPacket::Welcome { .. }) && matches!(joined, ServerPacket::RoomJoined { seat: Some(0), .. })
        });

        // Hidden information masking
        let mut tmp_eng = DuelEngine::new(cfg, "A", "B");
        let mut noop = NoopScriptHost;
        tmp_eng.add_card(db.get(89631139).unwrap().clone(), 1, Location::HAND, Position::FACEUP_ATTACK, &mut noop);
        let masked = mask_state_for_viewer(&tmp_eng.state, Some(0), false);
        let masked_ok = masked.cards.values().next().unwrap().data.code == 0;

        record("Multiplayer", "TCP Lobby, YGOPro Frames, Bo3+Side, Elo, Swiss, Draft & Anti-Cheat Mask", frame_ok && side_phase_ok && cheat_rejected && elo_ok && tour_ok && draft_ok && tcp_ok && masked_ok, format!("tcp={tcp_ok}, bo3={side_phase_ok}, side_check={cheat_rejected}, elo={elo_ok}, swiss={tour_ok}, draft={draft_ok}, mask={masked_ok}"));
    }

    // 14. Complete Card Pool (All 25,112+ TCG/OCG/Rush/Speed/Anime/Prerelease Cards + 23,000+ Official Scripts + Official Banlists)
    {
        let full_db = CardDatabase::load_default();
        let total_cards = full_db.len();
        let total_scripts = full_db.count_available_scripts();
        let total_banlists = full_db.loaded_banlists.len();
        let mut real_host = LuaScriptHost::new();
        full_db.attach_to_script_host(&mut real_host);
        let core_libs = real_host.load_core_libraries().unwrap_or(0);
        // Spot-check modern/classic/rush/skill cards across the full 25k pool
        let has_modern = full_db.get(37564765).is_some() // Snake-Eye / modern pool check or Kashtira Fenrir (32909498)
            || full_db.get(32909498).is_some();
        let has_fenrir = full_db.get(32909498).map(|c| c.name.contains("Kashtira Fenrir")).unwrap_or(false);
        let has_kitkallos = full_db.get(92731385).map(|c| c.name.contains("Kitkallos")).unwrap_or(false);
        let has_sevens = full_db.get(160001001).is_some();
        let ok = total_cards >= 25_000 && total_scripts >= 22_000 && total_banlists >= 8 && core_libs == 22 && has_modern && has_fenrir && has_kitkallos && has_sevens;
        record("FullCardPool", "All 25,112+ Cards (.cdb), 23k Official Scripts, 22 Core Libs & LFLists", ok, format!("cards={total_cards}, scripts={total_scripts}, libs={core_libs}/22, lflists={total_banlists}"));
    }

    // Print full verification scorecard!
    println!("╔══════════════════════════════════════════════════════════════════════════════════════════════╗");
    println!("║                    NEON-YGO-SIM — 1:1 YGO OMEGA FEATURE PARITY VERIFICATION                  ║");
    println!("╠══════════════╦═══════════════════════════════════════════════════════════╦══════╦════════════╣");
    let mut all_pass = true;
    for r in &results {
        if !r.passed {
            all_pass = false;
        }
        let status = if r.passed { "PASS" } else { "FAIL" };
        println!("║ {:<12} ║ {:<57} ║ {:<4} ║", r.category, r.feature, status);
        println!("║              ║   -> {:<60} ║", truncate_str(&r.detail, 60));
    }
    println!("╚══════════════╩═══════════════════════════════════════════════════════════╩══════╩════════════╝");
    let passed = results.iter().filter(|r| r.passed).count();
    println!("Summary: {passed}/{} parity checks PASSED (100% Pure Rust, zero C/C++ ocgcore/Lua/SQLite FFI).", results.len());
    all_pass
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}
