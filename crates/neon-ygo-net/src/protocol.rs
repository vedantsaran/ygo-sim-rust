use serde::{Deserialize, Serialize};
use thiserror::Error;
use neon_ygo_core::*;
use neon_ygo_db::Deck;

/// Official YGOPro / EDOPro / YGO Omega Client-to-Server packet opcodes (`CTOS_*`).
pub mod ctos {
    pub const RESPONSE: u8 = 0x01;
    pub const UPDATE_DECK: u8 = 0x02;
    pub const HAND_RESULT: u8 = 0x03;
    pub const TP_RESULT: u8 = 0x04;
    pub const PLAYER_INFO: u8 = 0x10;
    pub const CREATE_GAME: u8 = 0x11;
    pub const JOIN_GAME: u8 = 0x12;
    pub const LEAVE_GAME: u8 = 0x13;
    pub const SURRENDER: u8 = 0x14;
    pub const TIME_CONFIRM: u8 = 0x15;
    pub const CHAT: u8 = 0x16;
    pub const HS_TODUELIST: u8 = 0x20;
    pub const HS_TOOBSERVER: u8 = 0x21;
    pub const HS_READY: u8 = 0x22;
    pub const HS_NOTREADY: u8 = 0x23;
    pub const HS_KICK: u8 = 0x24;
    pub const HS_START: u8 = 0x25;
    pub const OMEGA_JSON: u8 = 0xF0;
}

/// Official YGOPro / EDOPro / YGO Omega Server-to-Client packet opcodes (`STOC_*`).
pub mod stoc {
    pub const GAME_MSG: u8 = 0x01;
    pub const ERROR_MSG: u8 = 0x02;
    pub const SELECT_HAND: u8 = 0x03;
    pub const SELECT_TP: u8 = 0x04;
    pub const HAND_RESULT: u8 = 0x05;
    pub const TP_RESULT: u8 = 0x06;
    pub const CHANGE_SIDE: u8 = 0x07;
    pub const WAITING_SIDE: u8 = 0x08;
    pub const CREATE_GAME: u8 = 0x11;
    pub const JOIN_GAME: u8 = 0x12;
    pub const TYPE_CHANGE: u8 = 0x13;
    pub const LEAVE_GAME: u8 = 0x14;
    pub const DUEL_START: u8 = 0x15;
    pub const DUEL_END: u8 = 0x16;
    pub const REPLAY: u8 = 0x17;
    pub const TIME_LIMIT: u8 = 0x18;
    pub const CHAT: u8 = 0x19;
    pub const HS_PLAYER_ENTER: u8 = 0x20;
    pub const HS_PLAYER_CHANGE: u8 = 0x21;
    pub const HS_WATCH_CHANGE: u8 = 0x22;
    pub const OMEGA_JSON: u8 = 0xF0;
}

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON codec error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("frame truncated or invalid")]
    InvalidFrame,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RoomMode {
    Single,
    MatchBo3,
    Tag2v2,
    Relay,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomSummary {
    pub room_id: String,
    pub title: String,
    pub format: FormatPreset,
    pub mode: RoomMode,
    pub has_passcode: bool,
    pub players: Vec<String>,
    pub spectators: usize,
    pub in_progress: bool,
    pub manual_mode: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientPacket {
    Hello { player_name: String },
    ListRooms,
    CreateRoom {
        title: String,
        passcode: Option<String>,
        format: FormatPreset,
        mode: RoomMode,
        manual_mode: bool,
    },
    JoinRoom {
        room_id: String,
        passcode: Option<String>,
        as_spectator: bool,
    },
    LeaveRoom,
    SubmitDeck(Deck),
    Ready(bool),
    Rps(RpsMove),
    ChooseTurnOrder { go_first: bool },
    DuelDecision(Decision),
    ManualCommand(ManualCommand),
    Chat(String),
    QueueRanked { format: FormatPreset, deck: Deck },
    LeaveRankedQueue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerPacket {
    Welcome { player_name: String },
    RoomList(Vec<RoomSummary>),
    RoomJoined { room: RoomSummary, seat: Option<PlayerId> },
    RoomUpdated(RoomSummary),
    Chat { sender: String, text: String },
    PromptRps,
    PromptTurnOrder,
    PromptSideDeck { game_number: u8 },
    DuelStateSync { state: DuelState, messages: Vec<DuelMessage> },
    MatchFinished { winner: Option<String>, score: [u8; 2] },
    RankedMatched { room_id: String, opponent: String, opponent_elo: i32 },
    Error(String),
}

/// Encode a packet into official YGOPro length-prefixed binary frame:
/// `[len_u16_le][opcode_u8][payload_bytes...]`.
pub fn encode_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let total_len = (1 + payload.len()) as u16;
    let mut buf = Vec::with_capacity(2 + 1 + payload.len());
    buf.extend_from_slice(&total_len.to_le_bytes());
    buf.push(opcode);
    buf.extend_from_slice(payload);
    buf
}

/// Decode a single YGOPro length-prefixed binary frame.
pub fn decode_frame(bytes: &[u8]) -> Result<(u8, Vec<u8>, usize), ProtocolError> {
    if bytes.len() < 3 {
        return Err(ProtocolError::InvalidFrame);
    }
    let body_len = u16::from_le_bytes([bytes[0], bytes[1]]) as usize;
    if body_len < 1 || bytes.len() < 2 + body_len {
        return Err(ProtocolError::InvalidFrame);
    }
    let opcode = bytes[2];
    let payload = bytes[3..2 + body_len].to_vec();
    Ok((opcode, payload, 2 + body_len))
}

pub fn encode_client_packet(pkt: &ClientPacket) -> Result<Vec<u8>, ProtocolError> {
    let json = serde_json::to_vec(pkt)?;
    let len = (1 + json.len()) as u32;
    let mut out = Vec::with_capacity(5 + json.len());
    out.extend_from_slice(&len.to_le_bytes());
    out.push(ctos::OMEGA_JSON);
    out.extend_from_slice(&json);
    Ok(out)
}

pub fn encode_server_packet(pkt: &ServerPacket) -> Result<Vec<u8>, ProtocolError> {
    let json = serde_json::to_vec(pkt)?;
    let len = (1 + json.len()) as u32;
    let mut out = Vec::with_capacity(5 + json.len());
    out.extend_from_slice(&len.to_le_bytes());
    out.push(stoc::OMEGA_JSON);
    out.extend_from_slice(&json);
    Ok(out)
}
