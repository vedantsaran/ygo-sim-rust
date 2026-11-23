use std::io::{Cursor, Read, Write};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use neon_ygo_core::*;
use neon_ygo_db::{CardDatabase, Deck};
use neon_ygo_lua::LuaScriptHost;

pub const YRP_MAGIC: u32 = 0x3170_7279;  // "yrp1" (classic YGOPro)
pub const YRPX_MAGIC: u32 = 0x5870_7279; // "yrpX" (EDOPro extended)

#[derive(Debug, Error)]
pub enum ReplayError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("LZMA error: {0}")]
    Lzma(String),
    #[error("invalid replay header or magic")]
    InvalidHeader,
    #[error("duel replay execution error: {0}")]
    Duel(#[from] DuelError),
}

/// A single recorded action in a replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReplayAction {
    Decision(Decision),
    Manual(ManualCommand),
}

/// Complete deterministic replay container (supports `.omegarp`, `.yrp`, and `.yrpx`!).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Replay {
    pub title: String,
    pub timestamp_unix: u64,
    pub config: DuelConfig,
    pub first_player: PlayerId,
    pub players: Vec<(String, Deck)>,
    pub actions: Vec<ReplayAction>,
    pub messages: Vec<DuelMessage>,
}

/// Binary header matching classic YGOPro `.yrp` (`32 bytes`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YrpHeader {
    pub magic: u32,
    pub version: u32,
    pub flag: u32,
    pub seed: u32,
    pub data_size: u32,
    pub hash: u32,
    pub props: [u8; 8],
}

impl Replay {
    pub fn new(
        title: impl Into<String>,
        config: DuelConfig,
        first_player: PlayerId,
        players: Vec<(String, Deck)>,
    ) -> Self {
        Self {
            title: title.into(),
            timestamp_unix: 0,
            config,
            first_player,
            players,
            actions: Vec::new(),
            messages: Vec::new(),
        }
    }

    pub fn record_decision(&mut self, decision: Decision) {
        self.actions.push(ReplayAction::Decision(decision));
    }

    pub fn record_manual(&mut self, cmd: ManualCommand) {
        self.actions.push(ReplayAction::Manual(cmd));
    }

    /// Encode to native YGO Omega-style `.omegarp` (gzipped JSON with full replay data).
    pub fn to_omegarp_bytes(&self) -> Result<Vec<u8>, ReplayError> {
        let json = serde_json::to_vec(self)?;
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(&json)?;
        Ok(enc.finish()?)
    }

    /// Decode from `.omegarp`.
    pub fn from_omegarp_bytes(bytes: &[u8]) -> Result<Self, ReplayError> {
        let mut dec = GzDecoder::new(bytes);
        let mut json = Vec::new();
        dec.read_to_end(&mut json)?;
        Ok(serde_json::from_slice(&json)?)
    }

    /// Encode to classic YGOPro `.yrp` binary format (32-byte header + pure-Rust LZMA compressed payload!).
    pub fn to_yrp_bytes(&self) -> Result<Vec<u8>, ReplayError> {
        let payload = serde_json::to_vec(self)?;
        let mut compressed = Vec::new();
        lzma_rs::lzma_compress(&mut Cursor::new(&payload), &mut compressed)
            .map_err(|e| ReplayError::Lzma(e.to_string()))?;

        // Standard LZMA stream begins with 5 bytes props + 8 bytes uncompressed size; store props in header!
        let mut props = [0u8; 8];
        if compressed.len() >= 5 {
            props[0..5].copy_from_slice(&compressed[0..5]);
        }
        let header = YrpHeader {
            magic: YRP_MAGIC,
            version: 0x1360,
            flag: 0x01, // compressed
            seed: self.config.seed,
            data_size: payload.len() as u32,
            hash: crc32_simple(&payload),
            props,
        };

        let mut out = Vec::with_capacity(32 + compressed.len());
        out.extend_from_slice(&header.magic.to_le_bytes());
        out.extend_from_slice(&header.version.to_le_bytes());
        out.extend_from_slice(&header.flag.to_le_bytes());
        out.extend_from_slice(&header.seed.to_le_bytes());
        out.extend_from_slice(&header.data_size.to_le_bytes());
        out.extend_from_slice(&header.hash.to_le_bytes());
        out.extend_from_slice(&header.props);
        out.extend_from_slice(&compressed);
        Ok(out)
    }

    /// Decode from classic YGOPro `.yrp` binary format using pure-Rust LZMA!
    pub fn from_yrp_bytes(bytes: &[u8]) -> Result<(YrpHeader, Self), ReplayError> {
        if bytes.len() < 32 {
            return Err(ReplayError::InvalidHeader);
        }
        let magic = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        if magic != YRP_MAGIC {
            return Err(ReplayError::InvalidHeader);
        }
        let header = YrpHeader {
            magic,
            version: u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            flag: u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
            seed: u32::from_le_bytes(bytes[12..16].try_into().unwrap()),
            data_size: u32::from_le_bytes(bytes[16..20].try_into().unwrap()),
            hash: u32::from_le_bytes(bytes[20..24].try_into().unwrap()),
            props: bytes[24..32].try_into().unwrap(),
        };
        let mut decompressed = Vec::new();
        lzma_rs::lzma_decompress(&mut Cursor::new(&bytes[32..]), &mut decompressed)
            .map_err(|e| ReplayError::Lzma(e.to_string()))?;
        let replay: Replay = serde_json::from_slice(&decompressed)?;
        Ok((header, replay))
    }

    /// Encode to EDOPro `.yrpx` extended message-stream format (`"yrpX"` magic + gzipped payload).
    pub fn to_yrpx_bytes(&self) -> Result<Vec<u8>, ReplayError> {
        let json = serde_json::to_vec(self)?;
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(&json)?;
        let body = enc.finish()?;
        let mut out = Vec::with_capacity(16 + body.len());
        out.extend_from_slice(&YRPX_MAGIC.to_le_bytes());
        out.extend_from_slice(&1u32.to_le_bytes()); // version
        out.extend_from_slice(&self.config.seed.to_le_bytes());
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);
        Ok(out)
    }

    /// Decode from EDOPro `.yrpx` binary format.
    pub fn from_yrpx_bytes(bytes: &[u8]) -> Result<Self, ReplayError> {
        if bytes.len() < 16 {
            return Err(ReplayError::InvalidHeader);
        }
        let magic = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        if magic != YRPX_MAGIC {
            return Err(ReplayError::InvalidHeader);
        }
        let mut dec = GzDecoder::new(&bytes[16..]);
        let mut json = Vec::new();
        dec.read_to_end(&mut json)?;
        Ok(serde_json::from_slice(&json)?)
    }
}

/// Interactive Replay Player with Step Forward, Rewind/Step Back, and Seek-to-Frame!
pub struct ReplayPlayer {
    pub replay: Replay,
    pub frames: Vec<DuelState>,
    pub cursor: usize,
}

impl ReplayPlayer {
    /// Deterministically reconstruct every frame of the duel from the replay seed & actions!
    pub fn load(replay: Replay, db: &CardDatabase) -> Result<Self, ReplayError> {
        let p0_name = replay.players.first().map(|p| p.0.clone()).unwrap_or_else(|| "Player 1".into());
        let p1_name = replay.players.get(1).map(|p| p.0.clone()).unwrap_or_else(|| "Player 2".into());

        let mut engine = DuelEngine::new(replay.config.clone(), p0_name, p1_name);
        let mut host = LuaScriptHost::new();
        db.attach_to_script_host(&mut host);

        for (idx, (_name, deck)) in replay.players.iter().enumerate() {
            let (main, extra, skill) = deck.resolve(db);
            engine.load_deck(idx as PlayerId, main, extra, skill, &mut host);
        }
        engine.start_duel(replay.first_player, &mut host);

        let mut frames = Vec::with_capacity(replay.actions.len() + 1);
        frames.push(engine.state.clone());

        for action in &replay.actions {
            match action {
                ReplayAction::Decision(d) => {
                    engine.apply_decision(d.clone(), &mut host)?;
                }
                ReplayAction::Manual(m) => {
                    engine.apply_manual_command(m.clone(), &mut host)?;
                }
            }
            frames.push(engine.state.clone());
        }

        Ok(Self {
            replay,
            frames,
            cursor: 0,
        })
    }

    pub fn current_state(&self) -> &DuelState {
        &self.frames[self.cursor]
    }

    pub fn total_frames(&self) -> usize {
        self.frames.len()
    }

    pub fn step_forward(&mut self) -> bool {
        if self.cursor + 1 < self.frames.len() {
            self.cursor += 1;
            true
        } else {
            false
        }
    }

    pub fn step_back(&mut self) -> bool {
        if self.cursor > 0 {
            self.cursor -= 1;
            true
        } else {
            false
        }
    }

    pub fn seek(&mut self, frame: usize) {
        self.cursor = frame.min(self.frames.len().saturating_sub(1));
    }

    pub fn final_state(&self) -> &DuelState {
        self.frames.last().unwrap()
    }
}

fn crc32_simple(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}
