use std::collections::BTreeMap;
use serde::{Deserialize, Serialize};
use neon_ygo_core::*;
use crate::catalog::CardDatabase;
use crate::deck::Deck;

/// Official YGOPro / EDOPro / YGO Omega Banlist (`.lflist.conf` compatible + Genesys point support!).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Banlist {
    pub name: String,
    pub format: FormatPreset,
    /// `card_code -> max allowed copies (0 = Forbidden, 1 = Limited, 2 = Semi-Limited)`.
    pub limits: BTreeMap<CardCode, u8>,
    /// Optional Genesys format point cost overrides (`card_code -> points`).
    pub genesys_point_overrides: BTreeMap<CardCode, u32>,
    /// Genesys total deck point cap (0 = disabled).
    pub genesys_cap: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeckValidationReport {
    pub valid: bool,
    pub errors: Vec<String>,
    pub genesys_points_used: u32,
    pub genesys_points_cap: u32,
}

impl Banlist {
    /// Parse standard YGOPro / EDOPro / YGO Omega `.lflist.conf` format (may contain multiple `!BanlistName` sections).
    pub fn parse_lflist_conf(text: &str) -> Vec<Banlist> {
        let mut out = Vec::new();
        let mut current: Option<Banlist> = None;

        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with("$whitelist") {
                continue;
            }
            if let Some(name) = line.strip_prefix('!') {
                if let Some(prev) = current.take() {
                    out.push(prev);
                }
                let clean = name.trim().to_string();
                let fmt = infer_format_from_name(&clean);
                current = Some(Banlist {
                    name: clean,
                    format: fmt,
                    limits: BTreeMap::new(),
                    genesys_point_overrides: BTreeMap::new(),
                    genesys_cap: if fmt == FormatPreset::Genesys { 100 } else { 0 },
                });
                continue;
            }
            if let Some(ref mut bl) = current {
                // Format: `<passcode> <limit> [--comment]`
                let mut parts = line.split_whitespace();
                if let (Some(c_str), Some(l_str)) = (parts.next(), parts.next()) {
                    if let (Ok(code), Ok(lim)) = (c_str.parse::<CardCode>(), l_str.parse::<u8>()) {
                        bl.limits.insert(code, lim);
                    }
                }
            }
        }
        if let Some(last) = current {
            out.push(last);
        }
        out
    }

    /// Serialize to standard `.lflist.conf` section.
    pub fn to_lflist_conf(&self) -> String {
        let mut out = format!("!{}\n", self.name);
        for (&code, &lim) in &self.limits {
            out.push_str(&format!("{code} {lim}\n"));
        }
        out
    }

    pub fn max_copies(&self, alias_code: CardCode) -> u8 {
        self.limits.get(&alias_code).copied().unwrap_or(3)
    }

    /// Built-in official YGO Omega Banlist presets!
    pub fn preset(format: FormatPreset) -> Self {
        match format {
            FormatPreset::TcgAdvanced => Self {
                name: "TCG Advanced".into(),
                format,
                limits: BTreeMap::from([
                    (55144522, 0), // Pot of Greed (Forbidden)
                    (79571449, 0), // Graceful Charity (Forbidden)
                    (23434538, 0), // Maxx "C" (Forbidden in TCG!)
                    (5851097, 0),  // Vanity's Emptiness (Forbidden)
                    (84815190, 0), // Baronne de Fleur (Forbidden in TCG)
                    (4280258, 0),  // Apollousa (Forbidden in TCG)
                    (19613556, 0), // Heavy Storm (Forbidden in TCG)
                    (83764718, 1), // Monster Reborn (Limited)
                    (18144506, 1), // Harpie's Feather Duster (Limited)
                    (24224830, 1), // Called by the Grave (Limited)
                    (32807846, 1), // ROTA (Limited)
                    (73628505, 1), // Terraforming (Limited)
                    (82732705, 1), // Skill Drain (Limited)
                    (44362883, 1), // Branded Fusion (Limited)
                    (35261759, 2), // Pot of Desires (Semi-Limited)
                ]),
                genesys_point_overrides: BTreeMap::new(),
                genesys_cap: 0,
            },
            FormatPreset::Ocg => Self {
                name: "OCG".into(),
                format,
                limits: BTreeMap::from([
                    (55144522, 0), // Pot of Greed
                    (79571449, 0), // Graceful Charity
                    (5851097, 0),  // Vanity's Emptiness
                    (23434538, 2), // Maxx "C" (Semi-Limited in OCG!)
                    (19613556, 1), // Heavy Storm (Limited in OCG!)
                    (83764718, 1), // Monster Reborn
                    (18144506, 1), // Feather Duster
                    (24224830, 2), // Called by the Grave
                    (82732705, 1), // Skill Drain
                ]),
                genesys_point_overrides: BTreeMap::new(),
                genesys_cap: 0,
            },
            FormatPreset::MasterDuel => Self {
                name: "Master Duel".into(),
                format,
                limits: BTreeMap::from([
                    (55144522, 0),
                    (79571449, 0),
                    (5851097, 0),
                    (23434538, 3), // Maxx "C" legal in MD
                    (24224830, 2),
                    (18144506, 1),
                    (83764718, 1),
                    (82732705, 1),
                ]),
                genesys_point_overrides: BTreeMap::new(),
                genesys_cap: 0,
            },
            FormatPreset::Goat => Self {
                name: "GOAT (April 2005)".into(),
                format,
                limits: BTreeMap::from([
                    (12580477, 0), // Raigeki (Forbidden in 2005!)
                    (53129443, 0), // Dark Hole (Forbidden in 2005!)
                    (18144506, 0), // Feather Duster (Forbidden in 2005!)
                    (83764718, 0), // Monster Reborn (Forbidden in 2005!)
                    (511000001, 0), // Yata-Garasu (Forbidden!)
                    (55144522, 1), // Pot of Greed (Limited in GOAT!)
                    (79571449, 1), // Graceful Charity (Limited in GOAT!)
                    (72989439, 1), // BLS - Envoy (Limited in GOAT!)
                    (71413901, 1), // Breaker (Limited in GOAT!)
                    (26202165, 1), // Sangan (Limited in GOAT!)
                    (19613556, 1), // Heavy Storm (Limited in GOAT!)
                    (5318639, 1),  // MST (Limited in GOAT!)
                    (44095762, 1), // Mirror Force (Limited in GOAT!)
                    (53582587, 1), // Torrential Tribute (Limited in GOAT!)
                    (83555666, 1), // Ring of Destruction (Limited in GOAT!)
                ]),
                genesys_point_overrides: BTreeMap::new(),
                genesys_cap: 0,
            },
            FormatPreset::Edison => Self {
                name: "Edison (March 2010)".into(),
                format,
                limits: BTreeMap::from([
                    (55144522, 0),
                    (79571449, 0),
                    (12580477, 0),
                    (72989439, 0),
                    (19613556, 1),
                    (5318639, 1),
                    (44095762, 1),
                    (53582587, 1),
                    (41420027, 1), // Solemn Judgment Limited in Edison!
                    (40044918, 1), // Stratos Limited in Edison!
                    (26202165, 1),
                    (70095154, 2), // Cyber Dragon Semi-Limited in Edison!
                ]),
                genesys_point_overrides: BTreeMap::new(),
                genesys_cap: 0,
            },
            FormatPreset::Genesys => Self {
                name: "Genesys (100 Points)".into(),
                format,
                limits: BTreeMap::new(), // Genesys uses point cap instead of hard bans!
                genesys_point_overrides: BTreeMap::from([
                    (55144522, 50), // Pot of Greed = 50 pts
                    (79571449, 40), // Graceful Charity = 40 pts
                    (82732705, 30), // Skill Drain = 30 pts
                    (23434538, 50), // Maxx "C" = 50 pts
                    (44362883, 25), // Branded Fusion = 25 pts
                    (30241314, 25), // Macro Cosmos = 25 pts
                    (10045474, 10), // Infinite Impermanence = 10 pts
                    (14558127, 10), // Ash Blossom = 10 pts
                ]),
                genesys_cap: 100,
            },
            other => Self {
                name: format!("{other:?}"),
                format: other,
                limits: BTreeMap::new(),
                genesys_point_overrides: BTreeMap::new(),
                genesys_cap: 0,
            },
        }
    }

    /// Exhaustive deck legality validator (1:1 YGO Omega Deck Builder rules!):
    /// Checks Main/Extra/Side bounds, shared-alias 3-copy limit, banlist limits,
    /// Rush Duel Legend limit (max 1 Monster / 1 Spell / 1 Trap Legend),
    /// Genesys total point budget, and format Pendulum/Link restrictions!
    pub fn validate_deck(
        &self,
        deck: &Deck,
        db: &CardDatabase,
        config: &DuelConfig,
    ) -> DeckValidationReport {
        let mut errors = Vec::new();
        if deck.main.len() < config.min_main_deck || deck.main.len() > config.max_main_deck {
            errors.push(format!(
                "Main Deck size {} must be between {} and {}",
                deck.main.len(),
                config.min_main_deck,
                config.max_main_deck
            ));
        }
        if deck.extra.len() > config.max_extra_deck {
            errors.push(format!(
                "Extra Deck size {} exceeds maximum {}",
                deck.extra.len(),
                config.max_extra_deck
            ));
        }
        if deck.side.len() > config.max_side_deck {
            errors.push(format!(
                "Side Deck size {} exceeds maximum {}",
                deck.side.len(),
                config.max_side_deck
            ));
        }

        // Count copies across Main + Extra + Side using official ALIAS resolution!
        // (e.g., 2x Umi + 2x A Legendary Ocean = 4 copies of Umi -> ILLEGAL!)
        let mut alias_counts: BTreeMap<CardCode, u32> = BTreeMap::new();
        let mut legend_monsters = 0u32;
        let mut legend_spells = 0u32;
        let mut legend_traps = 0u32;
        let mut genesys_total = 0u32;

        for (zone_name, list) in [("Main", &deck.main), ("Extra", &deck.extra), ("Side", &deck.side)] {
            for &code in list {
                let card = db.get(code);
                let alias = card.map(|c| c.effective_alias()).unwrap_or(code);
                *alias_counts.entry(alias).or_insert(0) += 1;

                if let Some(c) = card {
                    // Zone placement check
                    if zone_name == "Main" && c.is_extra_deck() {
                        errors.push(format!("Extra Deck monster '{}' cannot be in Main Deck", c.name));
                    }
                    if zone_name == "Extra" && !c.is_extra_deck() {
                        errors.push(format!("Main Deck card '{}' cannot be in Extra Deck", c.name));
                    }
                    // No Pendulum/Link format restriction (e.g., Genesys / Retro)
                    if config.flags.contains(DuelFlags::NO_PENDULUM_OR_LINK)
                        && (c.is_pendulum() || c.is_link())
                    {
                        errors.push(format!("'{}' is illegal (Pendulum/Link disabled in this format)", c.name));
                    }
                    // Rush Duel Legend counts
                    if config.is_rush() && c.is_legend() {
                        if c.is_monster() { legend_monsters += 1; }
                        if c.is_spell() { legend_spells += 1; }
                        if c.is_trap() { legend_traps += 1; }
                    }
                    // Genesys point cost accumulation
                    let pts = self
                        .genesys_point_overrides
                        .get(&alias)
                        .copied()
                        .unwrap_or(c.genesys_points);
                    genesys_total += pts;
                }
            }
        }

        for (alias, count) in alias_counts {
            let max_allowed = self.max_copies(alias) as u32;
            if count > max_allowed {
                let name = db.get(alias).map(|c| c.name.as_str()).unwrap_or("Unknown Card");
                errors.push(format!(
                    "'{}' (#{alias}) has {count} copies (max allowed in {}: {max_allowed})",
                    name, self.name
                ));
            }
        }

        if config.is_rush() {
            if legend_monsters > 1 {
                errors.push(format!("Rush Duel allows at most 1 Legend Monster (found {legend_monsters})"));
            }
            if legend_spells > 1 {
                errors.push(format!("Rush Duel allows at most 1 Legend Spell (found {legend_spells})"));
            }
            if legend_traps > 1 {
                errors.push(format!("Rush Duel allows at most 1 Legend Trap (found {legend_traps})"));
            }
        }

        let cap = if self.genesys_cap > 0 {
            self.genesys_cap
        } else {
            config.genesys_point_limit
        };
        if cap > 0 && genesys_total > cap {
            errors.push(format!(
                "Genesys point total {genesys_total} exceeds cap {cap}"
            ));
        }

        DeckValidationReport {
            valid: errors.is_empty(),
            errors,
            genesys_points_used: genesys_total,
            genesys_points_cap: cap,
        }
    }
}

fn infer_format_from_name(name: &str) -> FormatPreset {
    let l = name.to_lowercase();
    if l.contains("goat") { FormatPreset::Goat }
    else if l.contains("edison") { FormatPreset::Edison }
    else if l.contains("rush") { FormatPreset::RushDuel }
    else if l.contains("speed") { FormatPreset::SpeedDuel }
    else if l.contains("genesys") { FormatPreset::Genesys }
    else if l.contains("master duel") { FormatPreset::MasterDuel }
    else if l.contains("ocg") { FormatPreset::Ocg }
    else { FormatPreset::TcgAdvanced }
}
