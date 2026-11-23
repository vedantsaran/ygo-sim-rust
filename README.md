<img src="assets/icon.svg" width="64" height="64" alt="">

# neon-ygo-sim

A **100% pure-Rust, full-level Yu-Gi-Oh! simulator** engineered for verifiable **1:1 feature parity with YGO Omega** — with zero C/C++ `ocgcore`, `liblua`, or `libsqlite3` FFI dependencies.

## Architecture (8-Crate Pure-Rust Workspace)

| Crate | Purpose |
| :--- | :--- |
| `crates/neon-ygo-core` | Complete `ocgcore`-equivalent rule engine: Master Rules 1–5, all summoning mechanics, 5-substep Damage Step, Chain/SEGOC, Missed Timing ("When" vs "If"), Hard/Soft/Oath OPT, Continuous Layer & Floodgates, Link Arrow & Extra Link (U-Link) geometry, Column mechanics, Rush/Speed/Turbo/Action/Tag 2v2/Relay/GOAT/Edison/Genesys rules, 4-mode Omega Chain Control, Manual/Judge Mode & Undo Rollback, deterministic MT19937 RNG. |
| `crates/neon-ygo-lua` | 100% pure-Rust Lua 5.4 lexer, recursive-descent parser, and VM with full `ocgcore` / EDOPro / YGO Omega scripting API (`GetID`, `Duel.*`, `Card.*`, `Effect.*`, `Group.*`, `Debug.*`, `Auxiliary`/`aux`, and all official constants). Executes real `.lua` card scripts and `Debug.*` puzzle scripts. |
| `crates/neon-ygo-db` | Built-in multi-archetype card catalog + real Lua scripts, **pure-Rust SQLite3 `.cdb` binary B-tree reader**, `.lflist.conf` banlist parser, TCG/OCG/Master Duel/GOAT/Edison/Speed/Rush/Genesys presets, `.ydk` / `ydke://` / **YGO Omega Deck Code** (Base64 + Gzip/Deflate) codec, shared-alias & Genesys point validation, multi-criteria Deck Builder search, and exact Hypergeometric Hand Probability calculator. |
| `crates/neon-ygo-replay` | Deterministic replay engine supporting classic YGOPro **`.yrp`** (pure-Rust LZMA), EDOPro **`.yrpx`**, and Omega-style **`.omegarp`** with frame-by-frame seek, step-forward, and rewind. |
| `crates/neon-ygo-net` | YGOPro/Omega binary framing (`CTOS_*` / `STOC_*`), async `tokio` TCP Room/Lobby server & client, passcode rooms, live spectators with anti-cheat hidden-information masking, Bo3 Match + RPS + side-deck multiset validation, seasonal Elo Ranked Ladder & Matchmaking Queue, Automated Swiss (OMW% tiebreakers) & Single-Elimination Tournaments, and Draft/Sealed Cube engine. |
| `crates/neon-ygo-ai` | Strategic heuristic Duel AI, Solo Hand Test & Combo Sandbox, and Lua `Debug.*` Puzzle Runner with built-in playable puzzles. |
| `crates/neon-ygo-gui` | Native pure-Rust desktop client (`eframe`/`egui`) with Duel Arena, 4-mode Chain Control bar + hotkeys, Manual/Judge toolbox, Deck Builder, Puzzle Mode, Hand Test, Ranked & Tournament viewers, Replay scrubber, and Playmat/Sleeve/Audio customization. |
| `crates/neon-ygo-cli` | Unified `neon-ygo` binary (`gui`, `play`, `ai-demo`, `server`, `deck`, `verify`) and exhaustive 1:1 YGO Omega integration test suite (`tests/omega_parity_suite.rs`). |

## Quick Start & Verification

```bash
# Sync/update the complete 25,112+ card DB (BabelCDB), 23,019+ Lua scripts (CardScripts), and official LFLists
cargo run -p neon-ygo-cli -- sync

# Audit all 25,112+ .cdb cards, all 23,019+ Lua scripts, 22 core libraries, and 1,000 real official scripts
cargo run -p neon-ygo-cli -- audit-corpus

# Search all 25,112+ TCG/OCG/Rush/Speed/Anime/Prerelease cards
cargo run -p neon-ygo-cli -- cards "Kashtira"

# Run the full 24-test 1:1 YGO Omega parity & ruling integration suite
cargo test --workspace

# Run the live 14-check 1:1 YGO Omega parity audit CLI
cargo run -p neon-ygo-cli -- verify

# Launch the native pure-Rust desktop GUI (loads all 25,112+ cards)
cargo run -p neon-ygo-cli -- gui

# Play an interactive CLI duel vs AI (formats: tcg, ocg, md, goat, edison, speed, rush, turbo, tag, relay, genesys)
cargo run -p neon-ygo-cli -- play tcg

# Watch a full AI vs AI match
cargo run -p neon-ygo-cli -- ai-demo

# Start the async multiplayer & ranked lobby server
cargo run -p neon-ygo-cli -- server 127.0.0.1:7911
```
