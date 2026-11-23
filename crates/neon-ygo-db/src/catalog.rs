use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use neon_ygo_core::*;
use neon_ygo_lua::LuaScriptHost;

use crate::banlist::Banlist;
use crate::deck::Deck;
use crate::sqlite_cdb::{load_cdb_file, CdbError};

/// Complete Card Database supporting ALL official `.cdb` SQLite3 files, ALL `.lflist.conf` banlists,
/// ALL official ProjectIgnis/EDOPro/YGO Omega Lua card scripts, AND built-in fallback catalog!
#[derive(Debug, Clone, Default)]
pub struct CardDatabase {
    pub cards: BTreeMap<CardCode, CardData>,
    pub lua_scripts: BTreeMap<CardCode, String>,
    pub script_dirs: Vec<PathBuf>,
    pub loaded_banlists: Vec<Banlist>,
    pub data_root: Option<PathBuf>,
}

impl CardDatabase {
    /// Load the full card database from `data/` if present (all 25,000+ TCG/OCG/Rush/Speed/Anime/Prerelease cards,
    /// all 23,000+ Lua scripts, and all official `.lflist.conf` banlists), with built-in catalog fallback!
    pub fn load_default() -> Self {
        for candidate in [PathBuf::from("data"), PathBuf::from("../data"), PathBuf::from("../../data")] {
            if candidate.join("cdb").is_dir() {
                if let Ok(db) = Self::load_from_data_dir(&candidate) {
                    return db;
                }
            }
        }
        Self::with_builtin()
    }

    /// Load EVERY `.cdb`, `.lflist.conf`, and Lua script directory from `data_dir` using 100% pure Rust!
    pub fn load_from_data_dir(data_dir: impl AsRef<Path>) -> Result<Self, CdbError> {
        let root = data_dir.as_ref().to_path_buf();
        let mut db = Self::with_builtin();
        db.data_root = Some(root.clone());

        // 1. Load all `.cdb` files in deterministic precedence order
        let cdb_dir = root.join("cdb");
        if cdb_dir.is_dir() {
            let mut cdb_files: Vec<PathBuf> = std::fs::read_dir(&cdb_dir)?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("cdb"))
                .collect();
            // Load base databases first, then official cards.cdb, then releases/prereleases last so newest overrides
            cdb_files.sort_by_key(|p| {
                let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                let prio = if name == "cards-unofficial.cdb" {
                    0
                } else if name == "cards-skills-unofficial.cdb" {
                    1
                } else if name == "cards-rush.cdb" {
                    2
                } else if name == "cards-skills.cdb" {
                    3
                } else if name == "cards.cdb" {
                    4
                } else if name == "goat-entries.cdb" {
                    5
                } else if name.starts_with("release-") {
                    6
                } else {
                    7
                };
                (prio, name.to_string())
            });

            for path in cdb_files {
                let loaded = load_cdb_file(&path)?;
                for mut c in loaded {
                    // Preserve any built-in Rush Maximum ATK or Genesys point metadata
                    if let Some(existing) = db.cards.get(&c.code) {
                        if c.maximum_atk.is_none() {
                            c.maximum_atk = existing.maximum_atk;
                        }
                        if c.genesys_points == 0 {
                            c.genesys_points = existing.genesys_points;
                        }
                    }
                    db.cards.insert(c.code, c);
                }
            }
        }

        // 2. Register all official script directories (official, rush, skill, pre-release, unofficial, goat, root)
        let scripts_root = root.join("scripts");
        if scripts_root.is_dir() {
            for sub in ["official", "rush", "skill", "pre-release", "unofficial", "goat", "pre-errata", ""] {
                let dir = if sub.is_empty() {
                    scripts_root.clone()
                } else {
                    scripts_root.join(sub)
                };
                if dir.is_dir() {
                    db.script_dirs.push(dir);
                }
            }
        }

        // 3. Load all official `.lflist.conf` banlists
        let lf_dir = root.join("lflists");
        if lf_dir.is_dir() {
            let mut lf_files: Vec<PathBuf> = std::fs::read_dir(&lf_dir)?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.file_name()
                        .and_then(|s| s.to_str())
                        .map(|n| n.ends_with(".lflist.conf"))
                        .unwrap_or(false)
                })
                .collect();
            lf_files.sort();
            for path in lf_files {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    db.loaded_banlists.extend(Banlist::parse_lflist_conf(&text));
                }
            }
        }

        Ok(db)
    }

    /// Load the rich built-in card database & Lua card scripts covering all Yu-Gi-Oh! eras & mechanics.
    pub fn with_builtin() -> Self {
        let mut db = Self::default();
        populate_builtin_cards(&mut db);
        populate_builtin_scripts(&mut db);
        db
    }

    /// Merge cards from an external YGOPro / EDOPro / YGO Omega `.cdb` SQLite3 file!
    pub fn load_cdb(&mut self, path: impl AsRef<Path>) -> Result<usize, CdbError> {
        let loaded = load_cdb_file(path)?;
        let count = loaded.len();
        for c in loaded {
            self.cards.insert(c.code, c);
        }
        Ok(count)
    }

    pub fn insert(&mut self, card: CardData) {
        self.cards.insert(card.code, card);
    }

    pub fn get(&self, code: CardCode) -> Option<&CardData> {
        self.cards.get(&code)
    }

    pub fn find_by_name(&self, name: &str) -> Option<&CardData> {
        let lower = name.to_lowercase();
        self.cards.values().find(|c| c.name.to_lowercase() == lower)
            .or_else(|| self.cards.values().find(|c| c.name.to_lowercase().contains(&lower)))
    }

    pub fn len(&self) -> usize {
        self.cards.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cards.is_empty()
    }

    /// Hydrate a `LuaScriptHost` with every card, built-in script, AND all external script directories (23,000+ scripts).
    pub fn attach_to_script_host(&self, host: &mut LuaScriptHost) {
        for c in self.cards.values() {
            host.register_card_data(c.clone());
        }
        for (&code, src) in &self.lua_scripts {
            host.register_script(code, src.clone());
        }
        for dir in &self.script_dirs {
            host.add_script_dir(dir.clone());
        }
        if let Some(ref root) = self.data_root {
            let scripts_root = root.join("scripts");
            if scripts_root.is_dir() {
                host.set_core_script_dir(scripts_root);
            }
        }
    }

    /// Count all `.lua` card scripts available across registered script directories + built-in scripts.
    pub fn count_available_scripts(&self) -> usize {
        let mut count = self.lua_scripts.len();
        for dir in &self.script_dirs {
            if let Ok(rd) = std::fs::read_dir(dir) {
                count += rd
                    .filter_map(|e| e.ok())
                    .filter(|e| {
                        let name = e.file_name();
                        let s = name.to_string_lossy();
                        s.starts_with('c') && s.ends_with(".lua")
                    })
                    .count();
            }
        }
        count
    }

    /// Prebuilt ready-to-play decks for every YGO Omega format!
    pub fn starter_decks(&self) -> Vec<Deck> {
        vec![
            Deck {
                name: "Blue-Eyes Chaos (TCG Advanced)".into(),
                main: repeat_codes(&[
                    (89631139, 3), // Blue-Eyes White Dragon
                    (38517737, 3), // Blue-Eyes Alternative White Dragon
                    (8240199, 3),  // Sage with Eyes of Blue
                    (14558127, 3), // Ash Blossom & Joyous Spring
                    (97268402, 3), // Effect Veiler
                    (27204311, 2), // Nibiru, the Primal Being
                    (94145021, 2), // Droll & Lock Bird
                    (55410871, 2), // Blue-Eyes Chaos MAX Dragon
                    (21082832, 2), // Chaos Form
                    (12580477, 1), // Raigeki
                    (18144506, 1), // Harpie's Feather Duster
                    (83764718, 1), // Monster Reborn
                    (5318639, 3),  // Mystical Space Typhoon
                    (24224830, 1), // Called by the Grave
                    (14087893, 3), // Book of Moon
                    (35261759, 2), // Pot of Desires
                    (10045474, 3), // Infinite Impermanence
                    (41420027, 2), // Solemn Judgment
                ]),
                extra: vec![
                    23995346, // Blue-Eyes Ultimate Dragon
                    44508094, // Stardust Dragon
                    84815190, // Baronne de Fleur
                    84013237, // Number 39: Utopia
                    90448279, // Divine Arsenal AA-ZEUS
                    86066372, // Accesscode Talker
                    1861629,  // Decode Talker
                    2857636,  // Knightmare Phoenix
                    38342335, // Knightmare Unicorn
                ],
                side: vec![
                    82732705, 82732705, // Skill Drain
                    30241314, 30241314, // Macro Cosmos
                    30459350, 30459350, // Imperial Iron Wall
                    40605147, 40605147, // Solemn Strike
                ],
                skill: None,
            },
            Deck {
                name: "Dark Magician Control (TCG Advanced)".into(),
                main: repeat_codes(&[
                    (46986414, 3), // Dark Magician
                    (38033121, 3), // Dark Magician Girl
                    (7084129, 3),  // Magician's Rod
                    (14558127, 3), // Ash Blossom & Joyous Spring
                    (97268402, 3), // Effect Veiler
                    (26202165, 3), // Sangan
                    (71413901, 3), // Breaker the Magical Warrior
                    (12580477, 1), // Raigeki
                    (53129443, 2), // Dark Hole
                    (18144506, 1), // Harpie's Feather Duster
                    (83764718, 1), // Monster Reborn
                    (5318639, 3),  // Mystical Space Typhoon
                    (24094653, 2), // Polymerization
                    (14087893, 3), // Book of Moon
                    (70368879, 3), // Upstart Goblin
                    (10045474, 3), // Infinite Impermanence
                ]),
                extra: vec![
                    98502113, // Dark Paladin
                    84013237, // Number 39: Utopia
                    1861629,  // Decode Talker
                    86066372, // Accesscode Talker
                    2857636,  // Knightmare Phoenix
                ],
                side: vec![
                    44095762, 44095762, // Mirror Force
                    53582587, 53582587, // Torrential Tribute
                ],
                skill: None,
            },
            Deck {
                name: "GOAT Control (April 2005)".into(),
                main: repeat_codes(&[
                    (72989439, 1), // Black Luster Soldier - Envoy of the Beginning
                    (71413901, 1), // Breaker the Magical Warrior
                    (26202165, 1), // Sangan
                    (34853266, 2), // Tsukuyomi
                    (54652250, 3), // Man-Eater Bug
                    (40640057, 3), // Kuriboh
                    (46986414, 3), // Dark Magician
                    (89631139, 3), // Blue-Eyes White Dragon
                    (55144522, 1), // Pot of Greed
                    (79571449, 1), // Graceful Charity
                    (19613556, 1), // Heavy Storm
                    (5318639, 1),  // Mystical Space Typhoon
                    (14087893, 3), // Book of Moon
                    (70368879, 3), // Upstart Goblin
                    (56747793, 3), // United We Stand
                    (44095762, 1), // Mirror Force
                    (53582587, 1), // Torrential Tribute
                    (83555666, 1), // Ring of Destruction
                    (41420027, 3), // Solemn Judgment
                    (94192409, 3), // Compulsory Evacuation Device
                    (82732705, 1), // Skill Drain
                ]),
                extra: vec![23995346, 98502113],
                side: vec![],
                skill: None,
            },
            Deck {
                name: "Rush Duel Maximum (Yggdrago)".into(),
                main: repeat_codes(&[
                    (160003001, 3), // Yggdrago the Sky Emperor [L]
                    (160003002, 3), // Yggdrago the Sky Emperor [C]
                    (160003003, 3), // Yggdrago the Sky Emperor [R]
                    (160001001, 3), // Sevens Road Magician
                    (89631139, 1),  // Legend Monster (Blue-Eyes)
                    (71413901, 3),
                    (26202165, 3),
                    (40640057, 3),
                    (34853266, 3),
                    (54652250, 3),
                    (8240199, 3),
                    (7084129, 3),
                    (5318639, 3),
                    (14087893, 3),
                ]),
                extra: vec![],
                side: vec![],
                skill: None,
            },
            Deck {
                name: "Speed Duel (Destiny Draw)".into(),
                main: repeat_codes(&[
                    (46986414, 3),
                    (38033121, 3),
                    (71413901, 3),
                    (26202165, 3),
                    (14087893, 3),
                    (5318639, 3),
                    (44095762, 2),
                ]),
                extra: vec![98502113],
                side: vec![],
                skill: Some(300101001), // Destiny Draw Skill!
            },
        ]
    }
}

fn repeat_codes(spec: &[(CardCode, usize)]) -> Vec<CardCode> {
    let mut out = Vec::new();
    for &(code, count) in spec {
        for _ in 0..count {
            out.push(code);
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn mk_monster(
    code: CardCode,
    name: &str,
    ctype: CardType,
    atk: i32,
    def: i32,
    level: u32,
    race: Race,
    attr: Attribute,
    setcodes: &[u16],
    desc: &str,
) -> CardData {
    CardData {
        code,
        alias: 0,
        setcodes: setcodes.to_vec(),
        card_type: CardType::MONSTER | ctype,
        atk,
        def,
        level,
        race,
        attribute: attr,
        lscale: 0,
        rscale: 0,
        link_markers: LinkMarker::empty(),
        ot: 3,
        category: Category::empty(),
        name: name.into(),
        desc: desc.into(),
        strings: vec![],
        maximum_atk: None,
        genesys_points: 0,
    }
}

fn mk_link(
    code: CardCode,
    name: &str,
    atk: i32,
    rating: u32,
    markers: LinkMarker,
    race: Race,
    attr: Attribute,
    setcodes: &[u16],
    desc: &str,
) -> CardData {
    let mut c = mk_monster(
        code,
        name,
        CardType::LINK | CardType::EFFECT,
        atk,
        0,
        rating,
        race,
        attr,
        setcodes,
        desc,
    );
    c.link_markers = markers;
    c
}

#[allow(clippy::too_many_arguments)]
fn mk_pendulum(
    code: CardCode,
    name: &str,
    ctype: CardType,
    atk: i32,
    def: i32,
    level: u32,
    lscale: u32,
    rscale: u32,
    race: Race,
    attr: Attribute,
    setcodes: &[u16],
    desc: &str,
) -> CardData {
    let mut c = mk_monster(code, name, CardType::PENDULUM | ctype, atk, def, level, race, attr, setcodes, desc);
    c.lscale = lscale;
    c.rscale = rscale;
    c
}

fn mk_st(code: CardCode, name: &str, ctype: CardType, setcodes: &[u16], genesys_pts: u32, desc: &str) -> CardData {
    CardData {
        code,
        alias: 0,
        setcodes: setcodes.to_vec(),
        card_type: ctype,
        atk: 0,
        def: 0,
        level: 0,
        race: Race::NONE,
        attribute: Attribute::NONE,
        lscale: 0,
        rscale: 0,
        link_markers: LinkMarker::empty(),
        ot: 3,
        category: Category::empty(),
        name: name.into(),
        desc: desc.into(),
        strings: vec![],
        maximum_atk: None,
        genesys_points: genesys_pts,
    }
}

fn populate_builtin_cards(db: &mut CardDatabase) {
    let list = vec![
        // --- Iconic Main Deck Monsters ---
        mk_monster(89631139, "Blue-Eyes White Dragon", CardType::NORMAL | CardType::LEGEND, 3000, 2500, 8, Race::DRAGON, Attribute::LIGHT, &[0xdd], "This legendary dragon is a powerful engine of destruction."),
        mk_monster(38517737, "Blue-Eyes Alternative White Dragon", CardType::EFFECT | CardType::SPSUMMON, 3000, 2500, 8, Race::DRAGON, Attribute::LIGHT, &[0xdd], "Must first be Special Summoned by revealing 'Blue-Eyes White Dragon'. Once per turn: destroy 1 opponent monster."),
        mk_monster(8240199, "Sage with Eyes of Blue", CardType::EFFECT | CardType::TUNER, 0, 1500, 1, Race::SPELLCASTER, Attribute::LIGHT, &[0xdd], "When Normal Summoned: Add 1 Level 1 LIGHT Tuner from Deck to hand."),
        mk_monster(46986414, "Dark Magician", CardType::NORMAL, 2500, 2100, 7, Race::SPELLCASTER, Attribute::DARK, &[0x10a2], "The ultimate wizard in terms of attack and defense."),
        mk_monster(38033121, "Dark Magician Girl", CardType::EFFECT, 2000, 1700, 6, Race::SPELLCASTER, Attribute::DARK, &[0x20a2], "Gains 300 ATK for every 'Dark Magician' in either GY."),
        mk_monster(7084129, "Magician's Rod", CardType::EFFECT, 1600, 100, 3, Race::SPELLCASTER, Attribute::DARK, &[0x10a2], "When Normal Summoned: Add 1 Spell/Trap mentioning Dark Magician from Deck to hand."),
        mk_monster(74677422, "Red-Eyes Black Dragon", CardType::NORMAL, 2400, 2000, 7, Race::DRAGON, Attribute::DARK, &[0x3b], "A ferocious dragon with a deadly attack."),
        mk_monster(40640057, "Kuriboh", CardType::EFFECT, 300, 200, 1, Race::FIEND, Attribute::DARK, &[0xa4], "Discard to take no battle damage from 1 attack."),
        mk_monster(26202165, "Sangan", CardType::EFFECT, 1000, 600, 3, Race::FIEND, Attribute::DARK, &[], "If sent from field to GY: Add 1 monster with 1500 or less ATK from Deck to hand."),
        mk_monster(78010363, "Witch of the Black Forest", CardType::EFFECT, 1100, 1200, 4, Race::SPELLCASTER, Attribute::DARK, &[], "If sent from field to GY: Add 1 monster with 1500 or less DEF from Deck to hand."),
        mk_monster(71413901, "Breaker the Magical Warrior", CardType::EFFECT, 1600, 1000, 4, Race::SPELLCASTER, Attribute::DARK, &[], "If Normal Summoned: Place 1 Spell Counter (+300 ATK). Remove 1 counter: destroy 1 Spell/Trap."),
        mk_monster(34853266, "Tsukuyomi", CardType::EFFECT | CardType::SPIRIT, 1100, 1400, 4, Race::SPELLCASTER, Attribute::DARK, &[], "Spirit Monster. Returns to hand in End Phase. Flip 1 monster face-down."),
        mk_monster(72989439, "Black Luster Soldier - Envoy of the Beginning", CardType::EFFECT | CardType::SPSUMMON, 3000, 2500, 8, Race::WARRIOR, Attribute::LIGHT, &[0x10cf], "Must first be Special Summoned by banishing 1 LIGHT and 1 DARK from GY."),
        mk_monster(70095154, "Cyber Dragon", CardType::EFFECT, 2100, 1600, 5, Race::MACHINE, Attribute::LIGHT, &[0x1093], "If only your opponent controls a monster, Special Summon from hand."),
        mk_monster(40044918, "Elemental HERO Stratos", CardType::EFFECT, 1800, 300, 4, Race::WARRIOR, Attribute::WIND, &[0x3008], "When Normal or Special Summoned: Add 1 HERO monster or destroy Spells/Traps."),
        mk_monster(63977008, "Junk Synchron", CardType::EFFECT | CardType::TUNER, 1300, 500, 3, Race::WARRIOR, Attribute::DARK, &[0x1017, 0x43], "When Normal Summoned: Target 1 Level 2 or lower monster in GY; Special Summon it."),
        mk_monster(54652250, "Man-Eater Bug", CardType::EFFECT | CardType::FLIP, 450, 600, 2, Race::INSECT, Attribute::EARTH, &[], "FLIP: Target 1 monster on the field; destroy it."),
        mk_monster(69884162, "Elemental HERO Neos Alius", CardType::EFFECT | CardType::GEMINI, 1900, 1300, 4, Race::WARRIOR, Attribute::LIGHT, &[0x3008], "Gemini Monster: Normal Summon again on field to gain its effect."),
        mk_monster(511000001, "Yata-Garasu", CardType::EFFECT | CardType::SPIRIT, 200, 100, 2, Race::FIEND, Attribute::WIND, &[], "Spirit Monster. Returns to hand during End Phase."),
        mk_monster(2129638, "A-Assault Core", CardType::EFFECT | CardType::UNION, 1900, 200, 4, Race::MACHINE, Attribute::LIGHT, &[0x140], "Union Monster: Once per turn equip/unequip to a LIGHT Machine."),
        mk_monster(21296502, "Toon Dark Magician", CardType::EFFECT | CardType::TOON, 2500, 2100, 7, Race::SPELLCASTER, Attribute::DARK, &[0x62], "Toon Monster. Can attack directly if Toon World is on field."),
        mk_monster(68468459, "Fallen of Albaz", CardType::EFFECT, 1800, 0, 4, Race::DRAGON, Attribute::DARK, &[0x15d], "If Normal/Special Summoned: Discard 1 card; Fusion Summon using opponent monster."),
        mk_monster(26077387, "Sky Striker Ace - Raye", CardType::EFFECT, 1500, 1500, 4, Race::WARRIOR, Attribute::DARK, &[0x1115], "Quick Effect: Tribute this card; Special Summon 1 Sky Striker Ace Link."),
        mk_monster(20001443, "Swordsoul of Mo Ye", CardType::EFFECT, 1700, 1800, 4, Race::WYRM, Attribute::WATER, &[0x16b], "If Normal/Special Summoned: Reveal 1 Swordsoul/Wyrm; Special Summon a Level 4 Tuner Token."),

        // --- Hand Traps & Competitive Staples ---
        mk_monster(14558127, "Ash Blossom & Joyous Spring", CardType::EFFECT | CardType::TUNER, 0, 1800, 3, Race::ZOMBIE, Attribute::FIRE, &[], "Quick Effect: Discard to negate an effect that adds from Deck, Special Summons from Deck, or sends from Deck to GY."),
        mk_monster(23434538, "Maxx \"C\"", CardType::EFFECT, 500, 200, 2, Race::INSECT, Attribute::EARTH, &[], "Quick Effect: Discard; each time opponent Special Summons this turn, draw 1 card."),
        mk_monster(97268402, "Effect Veiler", CardType::EFFECT | CardType::TUNER, 0, 0, 1, Race::SPELLCASTER, Attribute::LIGHT, &[], "Quick Effect (opp Main Phase): Send to GY to negate 1 face-up opponent monster's effects until End Phase."),
        mk_monster(27204311, "Nibiru, the Primal Being", CardType::EFFECT, 3000, 600, 11, Race::ROCK, Attribute::LIGHT, &[], "Quick Effect: If opponent summoned 5+ monsters this turn, Tribute all face-up monsters and Special Summon this card + Token."),
        mk_monster(94145021, "Droll & Lock Bird", CardType::EFFECT, 0, 0, 1, Race::SPELLCASTER, Attribute::WIND, &[], "Quick Effect: After opponent adds a card from Deck to hand outside Draw Phase, neither player can add cards from Deck to hand this turn."),

        // --- Exodia (All 5 Exact Pieces!) ---
        mk_monster(33396948, "Exodia the Forbidden One", CardType::EFFECT, 1000, 1000, 3, Race::SPELLCASTER, Attribute::DARK, &[0x40], "If you have all 5 Forbidden One pieces in your hand, you win the Duel!"),
        mk_monster(7902349, "Left Arm of the Forbidden One", CardType::NORMAL, 200, 300, 1, Race::SPELLCASTER, Attribute::DARK, &[0x40], "A forbidden left arm sealed by magic."),
        mk_monster(70903634, "Right Arm of the Forbidden One", CardType::NORMAL, 200, 300, 1, Race::SPELLCASTER, Attribute::DARK, &[0x40], "A forbidden right arm sealed by magic."),
        mk_monster(8124921, "Left Leg of the Forbidden One", CardType::NORMAL, 200, 300, 1, Race::SPELLCASTER, Attribute::DARK, &[0x40], "A forbidden left leg sealed by magic."),
        mk_monster(44519536, "Right Leg of the Forbidden One", CardType::NORMAL, 200, 300, 1, Race::SPELLCASTER, Attribute::DARK, &[0x40], "A forbidden right leg sealed by magic."),

        // --- Ritual Monsters ---
        mk_monster(55410871, "Blue-Eyes Chaos MAX Dragon", CardType::RITUAL | CardType::EFFECT, 4000, 0, 8, Race::DRAGON, Attribute::DARK, &[0xdd, 0xcf], "Ritual Summon with Chaos Form. Untargetable, indestructible by effects, double piercing battle damage!"),
        mk_monster(64631466, "Relinquished", CardType::RITUAL | CardType::EFFECT, 0, 0, 1, Race::SPELLCASTER, Attribute::DARK, &[], "Ritual Monster. Equip 1 opponent monster and gain its ATK/DEF."),

        // --- Pendulum Monsters ---
        mk_pendulum(16178681, "Odd-Eyes Pendulum Dragon", CardType::EFFECT, 2500, 2000, 7, 4, 4, Race::DRAGON, Attribute::DARK, &[0x99], "Scale 4. Deals double battle damage to monsters."),
        mk_pendulum(94415058, "Stargazer Magician", CardType::EFFECT, 1200, 2400, 5, 1, 1, Race::SPELLCASTER, Attribute::DARK, &[0x98], "Scale 1 Pendulum Magician."),
        mk_pendulum(20409757, "Timegazer Magician", CardType::EFFECT, 1200, 600, 3, 8, 8, Race::SPELLCASTER, Attribute::DARK, &[0x98], "Scale 8 Pendulum Magician."),
        mk_pendulum(76794549, "Astrograph Sorcerer", CardType::EFFECT, 2500, 2000, 7, 1, 1, Race::SPELLCASTER, Attribute::DARK, &[], "Scale 1. Special Summon from hand when a card you control is destroyed."),

        // --- Extra Deck: Fusion, Synchro, Xyz, Link ---
        mk_monster(23995346, "Blue-Eyes Ultimate Dragon", CardType::FUSION, 4500, 3800, 12, Race::DRAGON, Attribute::LIGHT, &[0xdd], "3 'Blue-Eyes White Dragon'."),
        mk_monster(98502113, "Dark Paladin", CardType::FUSION | CardType::EFFECT, 2900, 2400, 8, Race::SPELLCASTER, Attribute::DARK, &[0x10a2], "Dark Magician + Buster Blader. Negate Spell activations."),
        mk_monster(44146295, "Mirrorjade the Iceblade Dragon", CardType::FUSION | CardType::EFFECT, 3000, 2500, 8, Race::WYRM, Attribute::DARK, &[0x15d], "Fallen of Albaz + 1 Fusion/Synchro/Xyz/Link monster."),
        mk_monster(44508094, "Stardust Dragon", CardType::SYNCHRO | CardType::EFFECT, 2500, 2000, 8, Race::DRAGON, Attribute::WIND, &[0xa3], "1 Tuner + 1+ non-Tuner monsters. Tribute to negate destruction."),
        mk_monster(73580471, "Black Rose Dragon", CardType::SYNCHRO | CardType::EFFECT, 2400, 1800, 7, Race::DRAGON, Attribute::FIRE, &[0x123], "1 Tuner + 1+ non-Tuner monsters. When Synchro Summoned: destroy all cards on the field."),
        mk_monster(84815190, "Baronne de Fleur", CardType::SYNCHRO | CardType::EFFECT, 3000, 2400, 10, Race::WARRIOR, Attribute::WIND, &[], "1 Tuner + 1+ non-Tuner monsters. Once per turn destroy 1 card; once while face-up omni-negate!"),
        mk_monster(69248256, "Swordsoul Grandmaster - Chixiao", CardType::SYNCHRO | CardType::EFFECT, 2800, 1000, 8, Race::WYRM, Attribute::LIGHT, &[0x16b], "1 Tuner + 1+ non-Tuner Wyrm monsters."),
        mk_monster(84013237, "Number 39: Utopia", CardType::XYZ | CardType::EFFECT, 2500, 2000, 4, Race::WARRIOR, Attribute::LIGHT, &[0x48, 0x107f], "2 Level 4 monsters. Detach 1 material to negate an attack."),
        mk_monster(90448279, "Divine Arsenal AA-ZEUS - Sky Thunder", CardType::XYZ | CardType::EFFECT, 3000, 3000, 12, Race::MACHINE, Attribute::LIGHT, &[], "2 Level 12 monsters. Quick Effect: Detach 2 materials to send all other cards on field to GY."),
        mk_link(1861629, "Decode Talker", 2300, 3, LinkMarker::TOP | LinkMarker::BOTTOM_LEFT | LinkMarker::BOTTOM_RIGHT, Race::CYBERSE, Attribute::DARK, &[0x101], "2+ Effect Monsters. Gains 500 ATK for each monster it points to."),
        mk_link(86066372, "Accesscode Talker", 2300, 4, LinkMarker::TOP | LinkMarker::LEFT | LinkMarker::RIGHT | LinkMarker::BOTTOM, Race::CYBERSE, Attribute::DARK, &[0x101], "2+ Effect Monsters. Opponent cannot activate cards/effects in response."),
        mk_link(2857636, "Knightmare Phoenix", 1900, 2, LinkMarker::TOP | LinkMarker::RIGHT, Race::FIEND, Attribute::FIRE, &[0x112], "2 monsters with different names. Co-link to draw 1."),
        mk_link(38342335, "Knightmare Unicorn", 2200, 3, LinkMarker::LEFT | LinkMarker::RIGHT | LinkMarker::BOTTOM, Race::FIEND, Attribute::DARK, &[0x112], "2+ monsters with different names. Discard 1 to spin 1 card to Deck."),
        mk_link(75452921, "Knightmare Cerberus", 1600, 2, LinkMarker::LEFT | LinkMarker::TOP, Race::FIEND, Attribute::EARTH, &[0x112], "2 monsters with different names. Discard 1 to destroy 1 Special Summoned monster."),
        mk_link(5043010, "Firewall Dragon", 2500, 4, LinkMarker::TOP | LinkMarker::LEFT | LinkMarker::RIGHT | LinkMarker::BOTTOM, Race::CYBERSE, Attribute::LIGHT, &[0x18f], "2+ monsters. Quick Effect bounce monsters up to co-linked count."),
        mk_link(65741786, "I:P Masquerena", 800, 2, LinkMarker::BOTTOM_LEFT | LinkMarker::BOTTOM_RIGHT, Race::CYBERSE, Attribute::DARK, &[], "2 non-Link Monsters. Opponent's Main Phase: Link Summon."),
        mk_link(4280258, "Apollousa, Bow of the Goddess", 0, 4, LinkMarker::TOP | LinkMarker::BOTTOM_LEFT | LinkMarker::BOTTOM | LinkMarker::BOTTOM_RIGHT, Race::FAIRY, Attribute::WIND, &[], "2+ monsters with different names. 800 ATK per material; lose 800 ATK to negate monster effect."),
        mk_link(63288573, "Sky Striker Ace - Kagari", 1500, 1, LinkMarker::TOP_LEFT, Race::MACHINE, Attribute::FIRE, &[0x1115], "1 non-FIRE Sky Striker Ace monster. Add 1 Sky Striker Spell from GY to hand."),

        // --- Rush Duel Maximum Monsters & Sevens Road ---
        {
            let mut c = mk_monster(160003001, "Yggdrago the Sky Emperor [L]", CardType::EFFECT | CardType::MAXIMUM, 900, 0, 3, Race::CYBERSE, Attribute::LIGHT, &[], "Rush Duel Maximum Left Piece.");
            c.ot = 0x10;
            c
        },
        {
            let mut c = mk_monster(160003002, "Yggdrago the Sky Emperor [C]", CardType::EFFECT | CardType::MAXIMUM, 2000, 0, 10, Race::CYBERSE, Attribute::LIGHT, &[], "Rush Duel Maximum Center Piece. MAXIMUM ATK 4000!");
            c.ot = 0x10;
            c.maximum_atk = Some(4000);
            c
        },
        {
            let mut c = mk_monster(160003003, "Yggdrago the Sky Emperor [R]", CardType::EFFECT | CardType::MAXIMUM, 600, 0, 3, Race::CYBERSE, Attribute::LIGHT, &[], "Rush Duel Maximum Right Piece.");
            c.ot = 0x10;
            c
        },
        {
            let mut c = mk_monster(160001001, "Sevens Road Magician", CardType::EFFECT, 2100, 1500, 7, Race::SPELLCASTER, Attribute::DARK, &[], "Rush Duel Ace Monster.");
            c.ot = 0x10;
            c
        },

        // --- Speed Duel Skill Cards ---
        {
            let mut c = mk_st(300101001, "Destiny Draw (Skill)", CardType::SKILL, &[], 0, "Speed Duel Skill: Draw 1 card from your Deck.");
            c.ot = 8;
            c
        },
        {
            let mut c = mk_st(300101002, "Beatdown (Skill)", CardType::SKILL, &[], 0, "Speed Duel Skill: Boost your monsters' ATK.");
            c.ot = 8;
            c
        },

        // --- Iconic Spells ---
        mk_st(55144522, "Pot of Greed", CardType::SPELL, &[], 50, "Draw 2 cards."),
        mk_st(79571449, "Graceful Charity", CardType::SPELL, &[], 40, "Draw 3 cards, then discard 2 cards."),
        mk_st(83764718, "Monster Reborn", CardType::SPELL, &[], 15, "Target 1 monster in either GY; Special Summon it."),
        mk_st(12580477, "Raigeki", CardType::SPELL, &[], 10, "Destroy all monsters your opponent controls."),
        mk_st(53129443, "Dark Hole", CardType::SPELL, &[], 5, "Destroy all monsters on the field."),
        mk_st(18144506, "Harpie's Feather Duster", CardType::SPELL, &[], 15, "Destroy all Spells and Traps your opponent controls."),
        mk_st(19613556, "Heavy Storm", CardType::SPELL, &[], 20, "Destroy all Spells and Traps on the field."),
        mk_st(5318639, "Mystical Space Typhoon", CardType::SPELL | CardType::QUICKPLAY, &[], 0, "Target 1 Spell/Trap on the field; destroy that target."),
        mk_st(24094653, "Polymerization", CardType::SPELL, &[], 0, "Fusion Summon 1 Fusion Monster from your Extra Deck."),
        mk_st(44362883, "Branded Fusion", CardType::SPELL, &[0x15d], 25, "Fusion Summon 1 Fusion Monster that mentions Fallen of Albaz using monsters from hand, Deck, or field."),
        mk_st(21082832, "Chaos Form", CardType::SPELL | CardType::RITUAL, &[0xcf], 0, "Ritual Summon 1 'Chaos' or 'Black Luster Soldier' Ritual Monster."),
        mk_st(32807846, "Reinforcement of the Army", CardType::SPELL, &[], 10, "Add 1 Level 4 or lower Warrior monster from your Deck to your hand."),
        mk_st(73628505, "Terraforming", CardType::SPELL, &[], 15, "Add 1 Field Spell from your Deck to your hand."),
        mk_st(63166095, "Sky Striker Mobilize - Engage!", CardType::SPELL, &[0x115], 10, "If you control no monsters in MMZ: Add 1 Sky Striker card from Deck; if 3+ Spells in GY, draw 1."),
        mk_st(24224830, "Called by the Grave", CardType::SPELL | CardType::QUICKPLAY, &[], 20, "Target 1 monster in opponent's GY; banish it and negate its effects."),
        mk_st(14087893, "Book of Moon", CardType::SPELL | CardType::QUICKPLAY, &[], 0, "Target 1 face-up monster on the field; change it to face-down Defense Position."),
        mk_st(35261759, "Pot of Desires", CardType::SPELL, &[], 10, "Banish 10 cards from the top of your Deck, face-down; draw 2 cards."),
        mk_st(70368879, "Upstart Goblin", CardType::SPELL, &[], 5, "Draw 1 card, then your opponent gains 1000 LP."),
        mk_st(56747793, "United We Stand", CardType::SPELL | CardType::EQUIP, &[], 0, "Equipped monster gains 800 ATK/DEF for each face-up monster you control."),
        mk_st(81674782, "Dimensional Fissure", CardType::SPELL | CardType::CONTINUOUS, &[], 20, "Any monster sent to the GY is banished instead."),
        mk_st(47355498, "Necrovalley", CardType::SPELL | CardType::FIELD, &[], 15, "All Gravekeeper's gain 500 ATK/DEF. Cards in GY cannot be banished."),
        mk_st(22702055, "Umi", CardType::SPELL | CardType::FIELD, &[], 0, "All Fish/Sea Serpent/Thunder/Aqua gain 200 ATK/DEF."),
        {
            // Official Alias test card: "A Legendary Ocean" is always treated as "Umi" (shared 3-copy limit!)
            let mut alo = mk_st(295517, "A Legendary Ocean", CardType::SPELL | CardType::FIELD, &[], 0, "(This card's name is always treated as 'Umi'.)");
            alo.alias = 22702055;
            alo
        },

        // --- Iconic Traps & Floodgates & Counter Traps ---
        mk_st(82732705, "Skill Drain", CardType::TRAP | CardType::CONTINUOUS, &[], 30, "Pay 1000 LP. Negate the effects of all face-up monsters while they are face-up on the field."),
        mk_st(30241314, "Macro Cosmos", CardType::TRAP | CardType::CONTINUOUS, &[], 25, "Any card sent to the GY is banished instead."),
        mk_st(30459350, "Imperial Iron Wall", CardType::TRAP | CardType::CONTINUOUS, &[], 10, "Neither player can banish cards."),
        mk_st(5851097, "Vanity's Emptiness", CardType::TRAP | CardType::CONTINUOUS, &[], 40, "Neither player can Special Summon monsters."),
        mk_st(10045474, "Infinite Impermanence", CardType::TRAP, &[], 10, "Target 1 face-up opponent monster; negate its effects until End Phase. Can be activated from hand if you control no cards."),
        mk_st(41420027, "Solemn Judgment", CardType::TRAP | CardType::COUNTER, &[], 15, "Pay half your LP; negate a Spell/Trap activation or Summon and destroy it."),
        mk_st(84749824, "Solemn Warning", CardType::TRAP | CardType::COUNTER, &[], 10, "Pay 2000 LP; negate a Summon or Special Summon effect and destroy it."),
        mk_st(40605147, "Solemn Strike", CardType::TRAP | CardType::COUNTER, &[], 10, "Pay 1500 LP; negate a monster effect or Special Summon and destroy it."),
        mk_st(44095762, "Mirror Force", CardType::TRAP, &[], 0, "When opponent declares an attack: Destroy all opponent Attack Position monsters."),
        mk_st(53582587, "Torrential Tribute", CardType::TRAP, &[], 5, "When a monster(s) is Summoned: Destroy all monsters on the field."),
        mk_st(94192409, "Compulsory Evacuation Device", CardType::TRAP, &[], 0, "Target 1 monster on the field; return it to the hand."),
        mk_st(83555666, "Ring of Destruction", CardType::TRAP, &[], 10, "Destroy 1 face-up monster and inflict damage equal to its ATK."),
    ];

    for card in list {
        db.cards.insert(card.code, card);
    }
}

/// Real executable EDOPro / YGO Omega Lua card scripts executed by our pure-Rust Lua 5.4 VM!
fn populate_builtin_scripts(db: &mut CardDatabase) {
    let scripts: &[(CardCode, &str)] = &[
        // Pot of Greed (55144522) — Draw 2 cards
        (55144522, r#"
local s,id=GetID()
function s.initial_effect(c)
    local e1=Effect.CreateEffect(c)
    e1:SetCategory(CATEGORY_DRAW)
    e1:SetType(EFFECT_TYPE_ACTIVATE)
    e1:SetCode(EVENT_FREE_CHAIN)
    e1:SetTarget(s.target)
    e1:SetOperation(s.activate)
    c:RegisterEffect(e1)
end
function s.target(e,tp,eg,ep,ev,re,r,rp,chk)
    if chk==0 then return Duel.IsPlayerCanDraw(tp,2) end
    Duel.SetOperationInfo(0,CATEGORY_DRAW,nil,0,tp,2)
end
function s.activate(e,tp,eg,ep,ev,re,r,rp)
    Duel.Draw(tp,2,REASON_EFFECT)
end
"#),
        // Graceful Charity (79571449) — Draw 3, discard 2
        (79571449, r#"
local s,id=GetID()
function s.initial_effect(c)
    local e1=Effect.CreateEffect(c)
    e1:SetCategory(CATEGORY_DRAW)
    e1:SetType(EFFECT_TYPE_ACTIVATE)
    e1:SetCode(EVENT_FREE_CHAIN)
    e1:SetTarget(s.target)
    e1:SetOperation(s.activate)
    c:RegisterEffect(e1)
end
function s.target(e,tp,eg,ep,ev,re,r,rp,chk)
    if chk==0 then return Duel.IsPlayerCanDraw(tp,3) end
end
function s.activate(e,tp,eg,ep,ev,re,r,rp)
    if Duel.Draw(tp,3,REASON_EFFECT)==3 then
        Duel.DiscardHand(tp,nil,2,2,REASON_EFFECT|REASON_DISCARD)
    end
end
"#),
        // Upstart Goblin (70368879) — Draw 1, Opponent gains 1000 LP
        (70368879, r#"
local s,id=GetID()
function s.initial_effect(c)
    local e1=Effect.CreateEffect(c)
    e1:SetCategory(CATEGORY_DRAW|CATEGORY_RECOVER)
    e1:SetType(EFFECT_TYPE_ACTIVATE)
    e1:SetCode(EVENT_FREE_CHAIN)
    e1:SetTarget(s.target)
    e1:SetOperation(s.activate)
    c:RegisterEffect(e1)
end
function s.target(e,tp,eg,ep,ev,re,r,rp,chk)
    if chk==0 then return Duel.IsPlayerCanDraw(tp,1) end
end
function s.activate(e,tp,eg,ep,ev,re,r,rp)
    if Duel.Draw(tp,1,REASON_EFFECT)>0 then
        Duel.Recover(1-tp,1000,REASON_EFFECT)
    end
end
"#),
        // Raigeki (12580477) — Destroy all opponent monsters
        (12580477, r#"
local s,id=GetID()
function s.initial_effect(c)
    local e1=Effect.CreateEffect(c)
    e1:SetCategory(CATEGORY_DESTROY)
    e1:SetType(EFFECT_TYPE_ACTIVATE)
    e1:SetCode(EVENT_FREE_CHAIN)
    e1:SetTarget(s.target)
    e1:SetOperation(s.activate)
    c:RegisterEffect(e1)
end
function s.target(e,tp,eg,ep,ev,re,r,rp,chk)
    if chk==0 then return Duel.GetFieldGroupCount(tp,0,LOCATION_MZONE)>0 end
end
function s.activate(e,tp,eg,ep,ev,re,r,rp)
    local g=Duel.GetFieldGroup(tp,0,LOCATION_MZONE)
    Duel.Destroy(g,REASON_EFFECT)
end
"#),
        // Dark Hole (53129443) — Destroy all monsters on field
        (53129443, r#"
local s,id=GetID()
function s.initial_effect(c)
    local e1=Effect.CreateEffect(c)
    e1:SetCategory(CATEGORY_DESTROY)
    e1:SetType(EFFECT_TYPE_ACTIVATE)
    e1:SetCode(EVENT_FREE_CHAIN)
    e1:SetTarget(s.target)
    e1:SetOperation(s.activate)
    c:RegisterEffect(e1)
end
function s.target(e,tp,eg,ep,ev,re,r,rp,chk)
    if chk==0 then return Duel.GetFieldGroupCount(tp,LOCATION_MZONE,LOCATION_MZONE)>0 end
end
function s.activate(e,tp,eg,ep,ev,re,r,rp)
    local g=Duel.GetFieldGroup(tp,LOCATION_MZONE,LOCATION_MZONE)
    Duel.Destroy(g,REASON_EFFECT)
end
"#),
        // Harpie's Feather Duster (18144506) — Destroy all opponent Spells/Traps
        (18144506, r#"
local s,id=GetID()
function s.initial_effect(c)
    local e1=Effect.CreateEffect(c)
    e1:SetCategory(CATEGORY_DESTROY)
    e1:SetType(EFFECT_TYPE_ACTIVATE)
    e1:SetCode(EVENT_FREE_CHAIN)
    e1:SetTarget(s.target)
    e1:SetOperation(s.activate)
    c:RegisterEffect(e1)
end
function s.target(e,tp,eg,ep,ev,re,r,rp,chk)
    if chk==0 then return Duel.GetFieldGroupCount(tp,0,LOCATION_SZONE|LOCATION_FZONE|LOCATION_PZONE)>0 end
end
function s.activate(e,tp,eg,ep,ev,re,r,rp)
    local g=Duel.GetFieldGroup(tp,0,LOCATION_SZONE|LOCATION_FZONE|LOCATION_PZONE)
    Duel.Destroy(g,REASON_EFFECT)
end
"#),
        // Mystical Space Typhoon (5318639) — Quick-Play Spell: Target 1 Spell/Trap; destroy it
        (5318639, r#"
local s,id=GetID()
function s.initial_effect(c)
    local e1=Effect.CreateEffect(c)
    e1:SetCategory(CATEGORY_DESTROY)
    e1:SetType(EFFECT_TYPE_ACTIVATE)
    e1:SetProperty(EFFECT_FLAG_CARD_TARGET)
    e1:SetCode(EVENT_FREE_CHAIN)
    e1:SetTarget(s.target)
    e1:SetOperation(s.activate)
    c:RegisterEffect(e1)
end
function s.target(e,tp,eg,ep,ev,re,r,rp,chk)
    if chk==0 then return Duel.IsExistingTarget(nil,tp,LOCATION_SZONE|LOCATION_FZONE|LOCATION_PZONE,LOCATION_SZONE|LOCATION_FZONE|LOCATION_PZONE,1,e:GetHandler()) end
    Duel.SelectTarget(tp,nil,tp,LOCATION_SZONE|LOCATION_FZONE|LOCATION_PZONE,LOCATION_SZONE|LOCATION_FZONE|LOCATION_PZONE,1,1,e:GetHandler())
end
function s.activate(e,tp,eg,ep,ev,re,r,rp)
    local tc=Duel.GetFirstTarget()
    if tc and tc:IsRelateToEffect(e) then
        Duel.Destroy(tc,REASON_EFFECT)
    end
end
"#),
        // Monster Reborn (83764718) — Special Summon 1 monster from either GY (enforcing Proper Summon!)
        (83764718, r#"
local s,id=GetID()
function s.initial_effect(c)
    local e1=Effect.CreateEffect(c)
    e1:SetCategory(CATEGORY_SPECIAL_SUMMON)
    e1:SetType(EFFECT_TYPE_ACTIVATE)
    e1:SetProperty(EFFECT_FLAG_CARD_TARGET)
    e1:SetCode(EVENT_FREE_CHAIN)
    e1:SetTarget(s.target)
    e1:SetOperation(s.activate)
    c:RegisterEffect(e1)
end
function s.filter(c,e,tp)
    return c:IsCanBeSpecialSummoned(e,0,tp,false,false)
end
function s.target(e,tp,eg,ep,ev,re,r,rp,chk)
    if chk==0 then return Duel.GetLocationCount(tp,LOCATION_MZONE)>0 and Duel.IsExistingTarget(s.filter,tp,LOCATION_GRAVE,LOCATION_GRAVE,1,nil,e,tp) end
    Duel.SelectTarget(tp,s.filter,tp,LOCATION_GRAVE,LOCATION_GRAVE,1,1,nil,e,tp)
end
function s.activate(e,tp,eg,ep,ev,re,r,rp)
    local tc=Duel.GetFirstTarget()
    if tc and tc:IsRelateToEffect(e) then
        Duel.SpecialSummon(tc,0,tp,tp,false,false,POS_FACEUP_ATTACK)
    end
end
"#),
        // Book of Moon (14087893) — Flip 1 face-up monster face-down Defense
        (14087893, r#"
local s,id=GetID()
function s.initial_effect(c)
    local e1=Effect.CreateEffect(c)
    e1:SetCategory(CATEGORY_POSITION)
    e1:SetType(EFFECT_TYPE_ACTIVATE)
    e1:SetProperty(EFFECT_FLAG_CARD_TARGET)
    e1:SetCode(EVENT_FREE_CHAIN)
    e1:SetTarget(s.target)
    e1:SetOperation(s.activate)
    c:RegisterEffect(e1)
end
function s.filter(c)
    return c:IsFaceup() and not c:IsType(TYPE_LINK)
end
function s.target(e,tp,eg,ep,ev,re,r,rp,chk)
    if chk==0 then return Duel.IsExistingTarget(s.filter,tp,LOCATION_MZONE,LOCATION_MZONE,1,nil) end
    Duel.SelectTarget(tp,s.filter,tp,LOCATION_MZONE,LOCATION_MZONE,1,1,nil)
end
function s.activate(e,tp,eg,ep,ev,re,r,rp)
    local tc=Duel.GetFirstTarget()
    if tc and tc:IsFaceup() then
        Duel.ChangePosition(tc,POS_FACEDOWN_DEFENSE)
    end
end
"#),
        // Pot of Desires (35261759) — Banish 10 from Deck face-down; draw 2 (Hard OPT!)
        (35261759, r#"
local s,id=GetID()
function s.initial_effect(c)
    local e1=Effect.CreateEffect(c)
    e1:SetCategory(CATEGORY_DRAW)
    e1:SetType(EFFECT_TYPE_ACTIVATE)
    e1:SetCode(EVENT_FREE_CHAIN)
    e1:SetCountLimit(1,id|EFFECT_COUNT_CODE_OATH)
    e1:SetCost(s.cost)
    e1:SetTarget(s.target)
    e1:SetOperation(s.activate)
    c:RegisterEffect(e1)
end
function s.cost(e,tp,eg,ep,ev,re,r,rp,chk)
    if chk==0 then return Duel.GetFieldGroupCount(tp,LOCATION_DECK,0)>=12 end
    local g=Duel.GetFieldGroup(tp,LOCATION_DECK,0)
    local rm=g:Select(tp,10,10,nil)
    Duel.Remove(rm,POS_FACEDOWN_DEFENSE,REASON_COST)
end
function s.target(e,tp,eg,ep,ev,re,r,rp,chk)
    if chk==0 then return Duel.IsPlayerCanDraw(tp,2) end
end
function s.activate(e,tp,eg,ep,ev,re,r,rp)
    Duel.Draw(tp,2,REASON_EFFECT)
end
"#),
        // Skill Drain (82732705) — Pay 1000 LP; negate all face-up monsters on field!
        (82732705, r#"
local s,id=GetID()
function s.initial_effect(c)
    local e1=Effect.CreateEffect(c)
    e1:SetType(EFFECT_TYPE_ACTIVATE)
    e1:SetCode(EVENT_FREE_CHAIN)
    e1:SetCost(s.cost)
    c:RegisterEffect(e1)
    local e2=Effect.CreateEffect(c)
    e2:SetType(EFFECT_TYPE_FIELD)
    e2:SetRange(LOCATION_SZONE)
    e2:SetTargetRange(LOCATION_MZONE,LOCATION_MZONE)
    e2:SetCode(EFFECT_DISABLE)
    c:RegisterEffect(e2)
end
function s.cost(e,tp,eg,ep,ev,re,r,rp,chk)
    if chk==0 then return Duel.CheckLPCost(tp,1000) end
    Duel.PayLPCost(tp,1000)
end
"#),
        // Macro Cosmos (30241314) — Redirect ALL cards going to GY -> Banished!
        (30241314, r#"
local s,id=GetID()
function s.initial_effect(c)
    local e1=Effect.CreateEffect(c)
    e1:SetType(EFFECT_TYPE_ACTIVATE)
    e1:SetCode(EVENT_FREE_CHAIN)
    c:RegisterEffect(e1)
    local e2=Effect.CreateEffect(c)
    e2:SetType(EFFECT_TYPE_FIELD)
    e2:SetRange(LOCATION_SZONE)
    e2:SetTargetRange(0xff,0xff)
    e2:SetCode(EFFECT_TO_GRAVE_REDIRECT)
    e2:SetValue(LOCATION_REMOVED)
    c:RegisterEffect(e2)
end
"#),
        // Imperial Iron Wall (30459350) — Neither player can banish cards!
        (30459350, r#"
local s,id=GetID()
function s.initial_effect(c)
    local e1=Effect.CreateEffect(c)
    e1:SetType(EFFECT_TYPE_ACTIVATE)
    e1:SetCode(EVENT_FREE_CHAIN)
    c:RegisterEffect(e1)
    local e2=Effect.CreateEffect(c)
    e2:SetType(EFFECT_TYPE_FIELD)
    e2:SetProperty(EFFECT_FLAG_PLAYER_TARGET)
    e2:SetRange(LOCATION_SZONE)
    e2:SetTargetRange(1,1)
    e2:SetCode(EFFECT_CANNOT_REMOVE)
    c:RegisterEffect(e2)
end
"#),
        // Vanity's Emptiness (5851097) — Neither player can Special Summon!
        (5851097, r#"
local s,id=GetID()
function s.initial_effect(c)
    local e1=Effect.CreateEffect(c)
    e1:SetType(EFFECT_TYPE_ACTIVATE)
    e1:SetCode(EVENT_FREE_CHAIN)
    c:RegisterEffect(e1)
    local e2=Effect.CreateEffect(c)
    e2:SetType(EFFECT_TYPE_FIELD)
    e2:SetProperty(EFFECT_FLAG_PLAYER_TARGET)
    e2:SetRange(LOCATION_SZONE)
    e2:SetTargetRange(1,1)
    e2:SetCode(EFFECT_CANNOT_SPECIAL_SUMMON)
    c:RegisterEffect(e2)
end
"#),
        // Solemn Judgment (41420027) — Spell Speed 3 Counter Trap: Pay half LP, negate activation!
        (41420027, r#"
local s,id=GetID()
function s.initial_effect(c)
    local e1=Effect.CreateEffect(c)
    e1:SetCategory(CATEGORY_NEGATE|CATEGORY_DESTROY)
    e1:SetType(EFFECT_TYPE_ACTIVATE)
    e1:SetCode(EVENT_CHAINING)
    e1:SetCost(s.cost)
    e1:SetOperation(s.activate)
    c:RegisterEffect(e1)
end
function s.cost(e,tp,eg,ep,ev,re,r,rp,chk)
    if chk==0 then return true end
    Duel.PayLPCost(tp,math.floor(Duel.GetLP(tp)/2))
end
function s.activate(e,tp,eg,ep,ev,re,r,rp)
    local cur=Duel.GetCurrentChain()
    if cur>=2 then
        Duel.NegateActivation(cur-1)
    end
end
"#),
        // Infinite Impermanence (10045474) — Hand-trap when controlling no cards, negate 1 monster!
        (10045474, r#"
local s,id=GetID()
function s.initial_effect(c)
    local e1=Effect.CreateEffect(c)
    e1:SetCategory(CATEGORY_DISABLE)
    e1:SetType(EFFECT_TYPE_ACTIVATE)
    e1:SetProperty(EFFECT_FLAG_CARD_TARGET)
    e1:SetCode(EVENT_FREE_CHAIN)
    e1:SetTarget(s.target)
    e1:SetOperation(s.activate)
    c:RegisterEffect(e1)
    local e2=Effect.CreateEffect(c)
    e2:SetType(EFFECT_TYPE_SINGLE)
    e2:SetCode(EFFECT_TRAP_ACT_IN_HAND)
    e2:SetCondition(s.handcon)
    c:RegisterEffect(e2)
end
function s.handcon(e)
    return Duel.GetFieldGroupCount(e:GetHandlerPlayer(),LOCATION_ONFIELD,0)==0
end
function s.target(e,tp,eg,ep,ev,re,r,rp,chk)
    if chk==0 then return Duel.IsExistingTarget(Card.IsFaceup,tp,0,LOCATION_MZONE,1,nil) end
    Duel.SelectTarget(tp,Card.IsFaceup,tp,0,LOCATION_MZONE,1,1,nil)
end
function s.activate(e,tp,eg,ep,ev,re,r,rp)
    local tc=Duel.GetFirstTarget()
    if tc and tc:IsFaceup() then
        local e1=Effect.CreateEffect(e:GetHandler())
        e1:SetType(EFFECT_TYPE_SINGLE)
        e1:SetCode(EFFECT_DISABLE)
        e1:SetReset(RESET_EVENT|RESET_PHASE|PHASE_END)
        tc:RegisterEffect(e1)
    end
end
"#),
        // Blue-Eyes Chaos MAX Dragon (55410871) — Untargetable, Indestructible by Effects, Piercing!
        (55410871, r#"
local s,id=GetID()
function s.initial_effect(c)
    local e1=Effect.CreateEffect(c)
    e1:SetType(EFFECT_TYPE_SINGLE)
    e1:SetCode(EFFECT_CANNOT_BE_EFFECT_TARGET)
    e1:SetRange(LOCATION_MZONE)
    e1:SetValue(1)
    c:RegisterEffect(e1)
    local e2=Effect.CreateEffect(c)
    e2:SetType(EFFECT_TYPE_SINGLE)
    e2:SetCode(EFFECT_INDESTRUCTABLE_EFFECT)
    e2:SetRange(LOCATION_MZONE)
    e2:SetValue(1)
    c:RegisterEffect(e2)
    local e3=Effect.CreateEffect(c)
    e3:SetType(EFFECT_TYPE_SINGLE)
    e3:SetCode(EFFECT_PIERCE)
    e3:SetRange(LOCATION_MZONE)
    e3:SetValue(1)
    c:RegisterEffect(e3)
end
"#),
        // Man-Eater Bug (54652250) — FLIP: Destroy 1 opponent monster!
        (54652250, r#"
local s,id=GetID()
function s.initial_effect(c)
    local e1=Effect.CreateEffect(c)
    e1:SetCategory(CATEGORY_DESTROY)
    e1:SetType(EFFECT_TYPE_SINGLE|EFFECT_TYPE_FLIP)
    e1:SetOperation(s.operation)
    c:RegisterEffect(e1)
end
function s.operation(e,tp,eg,ep,ev,re,r,rp)
    local g=Duel.GetFieldGroup(tp,0,LOCATION_MZONE)
    local tc=g:GetFirst()
    if tc then
        Duel.Destroy(tc,REASON_EFFECT)
    end
end
"#),
        // Droll & Lock Bird (94145021) — Quick Effect: prevent adding from Deck to Hand for rest of turn!
        (94145021, r#"
local s,id=GetID()
function s.initial_effect(c)
    local e1=Effect.CreateEffect(c)
    e1:SetType(EFFECT_TYPE_QUICK_O)
    e1:SetCode(EVENT_FREE_CHAIN)
    e1:SetRange(LOCATION_HAND)
    e1:SetCost(s.cost)
    e1:SetOperation(s.operation)
    c:RegisterEffect(e1)
end
function s.cost(e,tp,eg,ep,ev,re,r,rp,chk)
    if chk==0 then return true end
    Duel.SendtoGrave(e:GetHandler(),REASON_COST)
end
function s.operation(e,tp,eg,ep,ev,re,r,rp)
    local e1=Effect.CreateEffect(e:GetHandler())
    e1:SetType(EFFECT_TYPE_FIELD)
    e1:SetProperty(EFFECT_FLAG_PLAYER_TARGET)
    e1:SetCode(EFFECT_CANNOT_TO_HAND)
    e1:SetTargetRange(1,1)
    e1:SetReset(RESET_PHASE|PHASE_END)
    Duel.RegisterEffect(e1,tp)
end
"#),
        // Speed Duel Skill: Destiny Draw (300101001)
        (300101001, r#"
local s,id=GetID()
function s.initial_effect(c)
    local e1=Effect.CreateEffect(c)
    e1:SetType(EFFECT_TYPE_ACTIVATE)
    e1:SetCode(EVENT_FREE_CHAIN)
    e1:SetOperation(s.activate)
    c:RegisterEffect(e1)
end
function s.activate(e,tp,eg,ep,ev,re,r,rp)
    Duel.Draw(tp,1,REASON_EFFECT)
end
"#),
    ];

    for &(code, src) in scripts {
        db.lua_scripts.insert(code, src.to_string());
    }
}
