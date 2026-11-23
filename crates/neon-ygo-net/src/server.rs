use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex};

use neon_ygo_core::*;
use neon_ygo_db::{CardDatabase, Deck};
use neon_ygo_lua::LuaScriptHost;

use crate::match_flow::mask_state_for_viewer;
use crate::protocol::*;
use crate::ranked::RankedLadder;

type ClientTx = mpsc::UnboundedSender<ServerPacket>;

struct RoomState {
    id: String,
    title: String,
    passcode: Option<String>,
    format: FormatPreset,
    mode: RoomMode,
    manual_mode: bool,
    players: Vec<(String, ClientTx, Option<Deck>, bool)>,
    spectators: Vec<(String, ClientTx)>,
    engine: Option<DuelEngine>,
}

impl RoomState {
    fn summary(&self) -> RoomSummary {
        RoomSummary {
            room_id: self.id.clone(),
            title: self.title.clone(),
            format: self.format,
            mode: self.mode,
            has_passcode: self.passcode.is_some(),
            players: self.players.iter().map(|p| p.0.clone()).collect(),
            spectators: self.spectators.len(),
            in_progress: self.engine.is_some(),
            manual_mode: self.manual_mode,
        }
    }

    fn broadcast_chat(&self, sender: &str, text: &str) {
        let pkt = ServerPacket::Chat {
            sender: sender.to_string(),
            text: text.to_string(),
        };
        for p in &self.players {
            let _ = p.1.send(pkt.clone());
        }
        for s in &self.spectators {
            let _ = s.1.send(pkt.clone());
        }
    }

    fn sync_duel(&mut self) {
        let Some(ref mut engine) = self.engine else {
            return;
        };
        let msgs = engine.drain_messages();
        for (seat, p) in self.players.iter().enumerate() {
            let masked = mask_state_for_viewer(&engine.state, Some(seat as PlayerId), false);
            let _ = p.1.send(ServerPacket::DuelStateSync {
                state: masked,
                messages: msgs.clone(),
            });
        }
        let spec_state = mask_state_for_viewer(&engine.state, None, false);
        for s in &self.spectators {
            let _ = s.1.send(ServerPacket::DuelStateSync {
                state: spec_state.clone(),
                messages: msgs.clone(),
            });
        }
    }
}

struct LobbyHub {
    db: CardDatabase,
    rooms: BTreeMap<String, RoomState>,
    next_room_num: u32,
    ranked: RankedLadder,
    ranked_waiters: BTreeMap<String, (ClientTx, Deck)>,
}

/// Async pure-Rust YGO Omega Multiplayer Server (Rooms, Spectators, Chat, Live Duels, Ranked Matchmaking).
pub struct OmegaServer {
    hub: Arc<Mutex<LobbyHub>>,
}

impl OmegaServer {
    pub fn new(db: CardDatabase) -> Self {
        Self {
            hub: Arc::new(Mutex::new(LobbyHub {
                db,
                rooms: BTreeMap::new(),
                next_room_num: 1,
                ranked: RankedLadder::new(1),
                ranked_waiters: BTreeMap::new(),
            })),
        }
    }

    /// Bind and accept connections on `addr` (use `"127.0.0.1:0"` for ephemeral test port).
    pub async fn bind(self, addr: &str) -> Result<(SocketAddr, tokio::task::JoinHandle<()>), ProtocolError> {
        let listener = TcpListener::bind(addr).await?;
        let local_addr = listener.local_addr()?;
        let hub = self.hub.clone();
        let handle = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let h = hub.clone();
                tokio::spawn(async move {
                    let _ = handle_connection(stream, h).await;
                });
            }
        });
        Ok((local_addr, handle))
    }
}

async fn handle_connection(stream: TcpStream, hub: Arc<Mutex<LobbyHub>>) -> Result<(), ProtocolError> {
    let (mut reader, mut writer) = stream.into_split();
    let (tx, mut rx) = mpsc::unbounded_channel::<ServerPacket>();

    let write_task = tokio::spawn(async move {
        while let Some(pkt) = rx.recv().await {
            if let Ok(bytes) = encode_server_packet(&pkt) {
                if writer.write_all(&bytes).await.is_err() {
                    break;
                }
            }
        }
    });

    let mut player_name = String::from("Duelist");
    let mut current_room: Option<String> = None;

    loop {
        let mut len_buf = [0u8; 4];
        if reader.read_exact(&mut len_buf).await.is_err() {
            break;
        }
        let body_len = u32::from_le_bytes(len_buf) as usize;
        if body_len < 1 || body_len > 4 * 1024 * 1024 {
            break;
        }
        let mut body = vec![0u8; body_len];
        if reader.read_exact(&mut body).await.is_err() {
            break;
        }
        if body[0] != ctos::OMEGA_JSON {
            continue;
        }
        let Ok(pkt) = serde_json::from_slice::<ClientPacket>(&body[1..]) else {
            let _ = tx.send(ServerPacket::Error("malformed packet".into()));
            continue;
        };

        let mut locked = hub.lock().await;
        match pkt {
            ClientPacket::Hello { player_name: name } => {
                player_name = name.clone();
                let _ = tx.send(ServerPacket::Welcome { player_name: name });
            }
            ClientPacket::ListRooms => {
                let list = locked.rooms.values().map(|r| r.summary()).collect();
                let _ = tx.send(ServerPacket::RoomList(list));
            }
            ClientPacket::CreateRoom {
                title,
                passcode,
                format,
                mode,
                manual_mode,
            } => {
                let rid = format!("ROOM-{}", locked.next_room_num);
                locked.next_room_num += 1;
                let room = RoomState {
                    id: rid.clone(),
                    title,
                    passcode,
                    format,
                    mode,
                    manual_mode,
                    players: vec![(player_name.clone(), tx.clone(), None, false)],
                    spectators: Vec::new(),
                    engine: None,
                };
                let summary = room.summary();
                locked.rooms.insert(rid.clone(), room);
                current_room = Some(rid);
                let _ = tx.send(ServerPacket::RoomJoined {
                    room: summary,
                    seat: Some(0),
                });
            }
            ClientPacket::JoinRoom {
                room_id,
                passcode,
                as_spectator,
            } => {
                if let Some(room) = locked.rooms.get_mut(&room_id) {
                    if room.passcode.is_some() && room.passcode != passcode {
                        let _ = tx.send(ServerPacket::Error("invalid room passcode".into()));
                        continue;
                    }
                    let max_seats = if room.mode == RoomMode::Tag2v2 { 4 } else { 2 };
                    let seat = if as_spectator || room.players.len() >= max_seats {
                        room.spectators.push((player_name.clone(), tx.clone()));
                        None
                    } else {
                        let s = room.players.len() as PlayerId;
                        room.players.push((player_name.clone(), tx.clone(), None, false));
                        Some(s)
                    };
                    let summary = room.summary();
                    current_room = Some(room_id);
                    let _ = tx.send(ServerPacket::RoomJoined {
                        room: summary.clone(),
                        seat,
                    });
                    for p in &room.players {
                        let _ = p.1.send(ServerPacket::RoomUpdated(summary.clone()));
                    }
                    if room.engine.is_some() {
                        room.sync_duel();
                    }
                } else {
                    let _ = tx.send(ServerPacket::Error("room not found".into()));
                }
            }
            ClientPacket::SubmitDeck(deck) => {
                if let Some(ref rid) = current_room {
                    if let Some(room) = locked.rooms.get_mut(rid) {
                        if let Some(p) = room.players.iter_mut().find(|p| p.0 == player_name) {
                            p.2 = Some(deck);
                        }
                    }
                }
            }
            ClientPacket::Ready(ready) => {
                if let Some(ref rid) = current_room {
                    let db_clone = locked.db.clone();
                    if let Some(room) = locked.rooms.get_mut(rid) {
                        if let Some(p) = room.players.iter_mut().find(|p| p.0 == player_name) {
                            p.3 = ready;
                        }
                        let needed = if room.mode == RoomMode::Tag2v2 { 4 } else { 2 };
                        if room.players.len() >= needed && room.players.iter().all(|p| p.3) && room.engine.is_none() {
                            let mut cfg = DuelConfig::from_preset(room.format, 0xA5A5_1337);
                            if room.manual_mode {
                                cfg.flags.insert(DuelFlags::MANUAL_MODE);
                            }
                            let mut eng = DuelEngine::new(
                                cfg,
                                room.players[0].0.clone(),
                                room.players[1].0.clone(),
                            );
                            let mut host = LuaScriptHost::new();
                            db_clone.attach_to_script_host(&mut host);
                            let fallback = db_clone.starter_decks().into_iter().next().unwrap_or_default();
                            for (seat, p) in room.players.iter().enumerate() {
                                let d = p.2.as_ref().unwrap_or(&fallback);
                                let (main, extra, skill) = d.resolve(&db_clone);
                                eng.load_deck(seat as PlayerId, main, extra, skill, &mut host);
                            }
                            eng.start_duel(0, &mut host);
                            room.engine = Some(eng);
                            room.sync_duel();
                        }
                    }
                }
            }
            ClientPacket::DuelDecision(decision) => {
                if let Some(ref rid) = current_room {
                    let db_clone = locked.db.clone();
                    if let Some(room) = locked.rooms.get_mut(rid) {
                        if let Some(ref mut eng) = room.engine {
                            let mut host = LuaScriptHost::new();
                            db_clone.attach_to_script_host(&mut host);
                            if let Err(e) = eng.apply_decision(decision, &mut host) {
                                let _ = tx.send(ServerPacket::Error(e.to_string()));
                            }
                        }
                        room.sync_duel();
                    }
                }
            }
            ClientPacket::ManualCommand(cmd) => {
                if let Some(ref rid) = current_room {
                    let db_clone = locked.db.clone();
                    if let Some(room) = locked.rooms.get_mut(rid) {
                        if let Some(ref mut eng) = room.engine {
                            let mut host = LuaScriptHost::new();
                            db_clone.attach_to_script_host(&mut host);
                            if let Err(e) = eng.apply_manual_command(cmd, &mut host) {
                                let _ = tx.send(ServerPacket::Error(e.to_string()));
                            }
                        }
                        room.sync_duel();
                    }
                }
            }
            ClientPacket::Chat(text) => {
                if let Some(ref rid) = current_room {
                    if let Some(room) = locked.rooms.get(rid) {
                        room.broadcast_chat(&player_name, &text);
                    }
                }
            }
            ClientPacket::QueueRanked { format, deck } => {
                locked.ranked_waiters.insert(player_name.clone(), (tx.clone(), deck));
                if let Some((p1, p2)) = locked.ranked.enqueue(format, player_name.clone(), 400) {
                    let rid = format!("RANKED-{}", locked.next_room_num);
                    locked.next_room_num += 1;
                    let elo1 = locked.ranked.rating_of(format, &p1);
                    let elo2 = locked.ranked.rating_of(format, &p2);
                    let w1 = locked.ranked_waiters.remove(&p1);
                    let w2 = locked.ranked_waiters.remove(&p2);
                    if let (Some((tx1, d1)), Some((tx2, d2))) = (w1, w2) {
                        let _ = tx1.send(ServerPacket::RankedMatched {
                            room_id: rid.clone(),
                            opponent: p2.clone(),
                            opponent_elo: elo2,
                        });
                        let _ = tx2.send(ServerPacket::RankedMatched {
                            room_id: rid.clone(),
                            opponent: p1.clone(),
                            opponent_elo: elo1,
                        });
                        let room = RoomState {
                            id: rid.clone(),
                            title: format!("Ranked {p1} vs {p2}"),
                            passcode: None,
                            format,
                            mode: RoomMode::MatchBo3,
                            manual_mode: false,
                            players: vec![
                                (p1, tx1, Some(d1), true),
                                (p2, tx2, Some(d2), true),
                            ],
                            spectators: Vec::new(),
                            engine: None,
                        };
                        locked.rooms.insert(rid, room);
                    }
                }
            }
            ClientPacket::LeaveRoom => {
                if let Some(rid) = current_room.take() {
                    if let Some(room) = locked.rooms.get_mut(&rid) {
                        room.players.retain(|p| p.0 != player_name);
                        room.spectators.retain(|s| s.0 != player_name);
                    }
                }
            }
            _ => {}
        }
    }

    write_task.abort();
    Ok(())
}

/// Async pure-Rust TCP client for connecting to an `OmegaServer`.
pub struct OmegaClient {
    stream: TcpStream,
}

impl OmegaClient {
    pub async fn connect(addr: SocketAddr) -> Result<Self, ProtocolError> {
        let stream = TcpStream::connect(addr).await?;
        Ok(Self { stream })
    }

    pub async fn send(&mut self, pkt: &ClientPacket) -> Result<(), ProtocolError> {
        let bytes = encode_client_packet(pkt)?;
        self.stream.write_all(&bytes).await?;
        Ok(())
    }

    pub async fn recv(&mut self) -> Result<ServerPacket, ProtocolError> {
        let mut len_buf = [0u8; 4];
        self.stream.read_exact(&mut len_buf).await?;
        let body_len = u32::from_le_bytes(len_buf) as usize;
        if body_len < 1 {
            return Err(ProtocolError::InvalidFrame);
        }
        let mut body = vec![0u8; body_len];
        self.stream.read_exact(&mut body).await?;
        if body[0] != stoc::OMEGA_JSON {
            return Err(ProtocolError::InvalidFrame);
        }
        Ok(serde_json::from_slice(&body[1..])?)
    }
}
