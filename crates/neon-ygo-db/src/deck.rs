use std::io::{Read, Write};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use flate2::read::{DeflateDecoder, GzDecoder, ZlibDecoder};
use flate2::write::GzEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use neon_ygo_core::CardCode;
use crate::catalog::CardDatabase;

#[derive(Debug, Error)]
pub enum DeckError {
    #[error("invalid deck format: {0}")]
    InvalidFormat(String),
    #[error("base64 decode error: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("compression error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deck {
    pub name: String,
    pub main: Vec<CardCode>,
    pub extra: Vec<CardCode>,
    pub side: Vec<CardCode>,
    pub skill: Option<CardCode>,
}

impl Deck {
    pub fn total_cards(&self) -> usize {
        self.main.len() + self.extra.len() + self.side.len()
    }

    /// Parse standard YGOPro / EDOPro / YGO Omega `.ydk` plain-text format.
    pub fn from_ydk(name: impl Into<String>, text: &str) -> Self {
        let mut deck = Deck {
            name: name.into(),
            ..Default::default()
        };
        let mut section = 0u8; // 0 = main, 1 = extra, 2 = side
        for raw_line in text.lines() {
            let line = raw_line.trim();
            if line.is_empty() {
                continue;
            }
            if line.eq_ignore_ascii_case("#main") {
                section = 0;
                continue;
            }
            if line.eq_ignore_ascii_case("#extra") {
                section = 1;
                continue;
            }
            if line.eq_ignore_ascii_case("!side") {
                section = 2;
                continue;
            }
            if line.starts_with('#') || line.starts_with('!') {
                continue;
            }
            if let Ok(code) = line.parse::<CardCode>() {
                match section {
                    0 => deck.main.push(code),
                    1 => deck.extra.push(code),
                    _ => deck.side.push(code),
                }
            }
        }
        deck
    }

    /// Serialize to standard `.ydk` format.
    pub fn to_ydk(&self) -> String {
        let mut out = String::from("#created by neon-ygo-sim\n#main\n");
        for &c in &self.main {
            out.push_str(&format!("{c}\n"));
        }
        out.push_str("#extra\n");
        for &c in &self.extra {
            out.push_str(&format!("{c}\n"));
        }
        out.push_str("!side\n");
        for &c in &self.side {
            out.push_str(&format!("{c}\n"));
        }
        out
    }

    /// Parse `ydke://<main_b64>!<extra_b64>!<side_b64>!` URI (YGO Omega & EDOPro standard).
    pub fn from_ydke(name: impl Into<String>, uri: &str) -> Result<Self, DeckError> {
        let trimmed = uri.trim();
        let body = trimmed
            .strip_prefix("ydke://")
            .ok_or_else(|| DeckError::InvalidFormat("missing ydke:// prefix".into()))?;
        let parts: Vec<&str> = body.split('!').collect();
        if parts.len() < 3 {
            return Err(DeckError::InvalidFormat("ydke URI must have 3 sections".into()));
        }
        Ok(Deck {
            name: name.into(),
            main: decode_u32_le_b64(parts[0])?,
            extra: decode_u32_le_b64(parts[1])?,
            side: decode_u32_le_b64(parts[2])?,
            skill: None,
        })
    }

    /// Encode to `ydke://` URI.
    pub fn to_ydke(&self) -> String {
        format!(
            "ydke://{}!{}!{}!",
            encode_u32_le_b64(&self.main),
            encode_u32_le_b64(&self.extra),
            encode_u32_le_b64(&self.side),
        )
    }

    /// Encode to **YGO Omega Deck Code** (2-byte header `[main_and_extra_count: u8, side_count: u8]`
    /// + little-endian `u32` card passcodes, Gzip-compressed in pure Rust, Base64-encoded).
    pub fn to_omega_code(&self) -> Result<String, DeckError> {
        let main_extra_count = (self.main.len() + self.extra.len()).min(255) as u8;
        let side_count = self.side.len().min(255) as u8;
        let mut raw = Vec::with_capacity(2 + 4 * (main_extra_count as usize + side_count as usize));
        raw.push(main_extra_count);
        raw.push(side_count);
        for &c in self.main.iter().chain(self.extra.iter()).take(main_extra_count as usize) {
            raw.extend_from_slice(&c.to_le_bytes());
        }
        for &c in self.side.iter().take(side_count as usize) {
            raw.extend_from_slice(&c.to_le_bytes());
        }
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&raw)?;
        let compressed = encoder.finish()?;
        Ok(BASE64.encode(compressed))
    }

    /// Decode a **YGO Omega Deck Code** (supports Gzip, Zlib, or Raw Deflate payloads!).
    /// Automatically splits Main Deck vs Extra Deck using `CardDatabase` metadata!
    pub fn from_omega_code(name: impl Into<String>, code: &str, db: &CardDatabase) -> Result<Self, DeckError> {
        let compressed = BASE64.decode(code.trim())?;
        let raw = decompress_any(&compressed)?;
        if raw.len() < 2 {
            return Err(DeckError::InvalidFormat("Omega deck payload too short".into()));
        }
        let main_extra_count = raw[0] as usize;
        let side_count = raw[1] as usize;
        let expected_bytes = 2 + 4 * (main_extra_count + side_count);
        if raw.len() < expected_bytes {
            return Err(DeckError::InvalidFormat(format!(
                "Omega deck truncated: expected {expected_bytes} bytes, got {}",
                raw.len()
            )));
        }

        let mut main = Vec::new();
        let mut extra = Vec::new();
        let mut side = Vec::new();
        let mut offset = 2usize;

        for _ in 0..main_extra_count {
            let cid = u32::from_le_bytes(raw[offset..offset + 4].try_into().unwrap());
            offset += 4;
            let is_ex = db.get(cid).map(|c| c.is_extra_deck()).unwrap_or(false);
            if is_ex {
                extra.push(cid);
            } else {
                main.push(cid);
            }
        }
        for _ in 0..side_count {
            let cid = u32::from_le_bytes(raw[offset..offset + 4].try_into().unwrap());
            offset += 4;
            side.push(cid);
        }

        Ok(Deck {
            name: name.into(),
            main,
            extra,
            side,
            skill: None,
        })
    }

    /// Resolve card passcodes into concrete `CardData` lists for `DuelEngine::load_deck`.
    pub fn resolve(&self, db: &CardDatabase) -> (Vec<neon_ygo_core::CardData>, Vec<neon_ygo_core::CardData>, Option<neon_ygo_core::CardData>) {
        let main = self.main.iter().filter_map(|c| db.get(*c).cloned()).collect();
        let extra = self.extra.iter().filter_map(|c| db.get(*c).cloned()).collect();
        let skill = self.skill.and_then(|c| db.get(c).cloned());
        (main, extra, skill)
    }
}

fn encode_u32_le_b64(codes: &[CardCode]) -> String {
    let mut bytes = Vec::with_capacity(codes.len() * 4);
    for &c in codes {
        bytes.extend_from_slice(&c.to_le_bytes());
    }
    BASE64.encode(bytes)
}

fn decode_u32_le_b64(s: &str) -> Result<Vec<CardCode>, DeckError> {
    if s.is_empty() {
        return Ok(Vec::new());
    }
    let bytes = BASE64.decode(s)?;
    if bytes.len() % 4 != 0 {
        return Err(DeckError::InvalidFormat("ydke segment length not multiple of 4".into()));
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|ch| u32::from_le_bytes([ch[0], ch[1], ch[2], ch[3]]))
        .collect())
}

fn decompress_any(bytes: &[u8]) -> Result<Vec<u8>, DeckError> {
    // 1. Try Gzip
    let mut out = Vec::new();
    if GzDecoder::new(bytes).read_to_end(&mut out).is_ok() {
        return Ok(out);
    }
    // 2. Try Zlib
    out.clear();
    if ZlibDecoder::new(bytes).read_to_end(&mut out).is_ok() {
        return Ok(out);
    }
    // 3. Try Raw Deflate
    out.clear();
    DeflateDecoder::new(bytes).read_to_end(&mut out)?;
    Ok(out)
}
