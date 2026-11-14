//! Локальный список друзей: `%APPDATA%\Albus\friends.json`.
//!
//! Без своего сервера: друзья хранятся локально (node_id, имя, дата добавления),
//! онлайн определяется dial'ом по NodeID через discovery N0, офлайн — по таймауту.
//! Не доставленная заявка остаётся в `outgoing` и переотправляется при следующем
//! соединении с адресатом.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

/// Входящая/исходящая заявка в друзья (по сети — отправитель, не адресат).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FriendRequest {
    pub sender_id: u64,
    pub node_id: String,
    pub nick: String,
}

/// Наша заявка, ожидающая подтверждения: помним, кому она адресована.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutgoingRequest {
    pub target_node_id: String,
    pub sender_id: u64,
    pub node_id: String,
    pub nick: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Friend {
    pub node_id: String,
    pub name: String,
    pub added_at: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct FriendsState {
    friends: Vec<Friend>,
    incoming: Vec<FriendRequest>,
    outgoing: Vec<OutgoingRequest>,
}

pub struct FriendsManager {
    state: Mutex<FriendsState>,
}

impl FriendsManager {
    pub fn load() -> Self {
        let state = Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        FriendsManager {
            state: Mutex::new(state),
        }
    }

    fn path() -> Option<PathBuf> {
        crate::config::albus_dir().map(|d| d.join("friends.json"))
    }

    fn save(&self) {
        let Some(path) = Self::path() else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let text = match serde_json::to_string_pretty(&*self.state.lock().unwrap()) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("[ДРУЗЬЯ] Не удалось сериализовать friends.json: {}", e);
                return;
            }
        };
        if let Err(e) = std::fs::write(&path, text) {
            eprintln!("[ДРУЗЬЯ] Не удалось сохранить friends.json: {}", e);
        }
    }

    pub fn friends(&self) -> Vec<Friend> {
        self.state.lock().unwrap().friends.clone()
    }

    pub fn incoming(&self) -> Vec<FriendRequest> {
        self.state.lock().unwrap().incoming.clone()
    }

    pub fn is_friend(&self, node_id: &str) -> bool {
        self.state
            .lock()
            .unwrap()
            .friends
            .iter()
            .any(|f| f.node_id == node_id)
    }

    /// Добавляет или обновляет друга и сохраняет список.
    pub fn add_friend(&self, node_id: String, name: String) {
        let mut s = self.state.lock().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        if let Some(f) = s.friends.iter_mut().find(|f| f.node_id == node_id) {
            f.name = name;
        } else {
            s.friends.push(Friend {
                node_id,
                name,
                added_at: now,
            });
        }
        drop(s);
        self.save();
    }

    /// Удаляет друга по NodeID; возвращает true, если друг был.
    pub fn remove_friend(&self, node_id: &str) -> bool {
        let mut s = self.state.lock().unwrap();
        let before = s.friends.len();
        s.friends.retain(|f| f.node_id != node_id);
        let removed = s.friends.len() != before;
        drop(s);
        if removed {
            self.save();
        }
        removed
    }

    /// Регистрирует входящую заявку (если не дубликат и уже не друг).
    pub fn add_incoming(&self, req: FriendRequest) {
        let mut s = self.state.lock().unwrap();
        if s.friends.iter().any(|f| f.node_id == req.node_id) {
            return;
        }
        if s.incoming.iter().any(|r| r.node_id == req.node_id) {
            return;
        }
        s.incoming.push(req);
        drop(s);
        self.save();
    }

    /// Принимает входящую заявку: переносит в друзья и возвращает её для ответа.
    pub fn accept_incoming(&self, node_id: &str) -> Option<FriendRequest> {
        let mut s = self.state.lock().unwrap();
        let pos = s.incoming.iter().position(|r| r.node_id == node_id)?;
        let req = s.incoming.remove(pos);
        s.friends.push(Friend {
            node_id: req.node_id.clone(),
            name: req.nick.clone(),
            added_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        });
        drop(s);
        self.save();
        Some(req)
    }

    pub fn decline_incoming(&self, node_id: &str) {
        let mut s = self.state.lock().unwrap();
        s.incoming.retain(|r| r.node_id != node_id);
        drop(s);
        self.save();
    }

    /// Запоминает нашу заявку, адресованную `target_node_id`.
    pub fn add_outgoing(&self, target_node_id: String, req: FriendRequest) {
        let mut s = self.state.lock().unwrap();
        let out = OutgoingRequest {
            target_node_id,
            sender_id: req.sender_id,
            node_id: req.node_id,
            nick: req.nick,
        };
        if !s.outgoing.iter().any(|o| o.target_node_id == out.target_node_id) {
            s.outgoing.push(out);
        }
        drop(s);
        self.save();
    }

    /// Заявки, адресованные конкретному узлу (для переотправки после соединения).
    pub fn pending_outgoing_to(&self, target_node_id: &str) -> Vec<FriendRequest> {
        self.state
            .lock()
            .unwrap()
            .outgoing
            .iter()
            .filter(|o| o.target_node_id == target_node_id)
            .map(|o| FriendRequest {
                sender_id: o.sender_id,
                node_id: o.node_id.clone(),
                nick: o.nick.clone(),
            })
            .collect()
    }

    /// Получив подтверждение от `peer_node_id`, добавляет его в друзья и снимает
    /// нашу ожидающую заявку, адресованную ему.
    pub fn resolve_outgoing(&self, peer_node_id: &str, peer_nick: &str) {
        let mut s = self.state.lock().unwrap();
        s.outgoing
            .retain(|o| o.target_node_id != peer_node_id);
        if !s.friends.iter().any(|f| f.node_id == peer_node_id) {
            s.friends.push(Friend {
                node_id: peer_node_id.to_string(),
                name: peer_nick.to_string(),
                added_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64,
            });
        }
        drop(s);
        self.save();
    }
}

/// Переотправляет не доставленные заявки узлу после установления соединения.
pub async fn resend_pending_to(
    friends: &FriendsManager,
    p2p: &crate::network::p2p_iroh::IrohP2pManager,
    target_node_id: &str,
) {
    for req in friends.pending_outgoing_to(target_node_id) {
        let msg = crate::network::NetworkMessage::FriendRequest {
            sender_id: req.sender_id,
            node_id: req.node_id,
            nick: req.nick,
        };
        let bytes = crate::network::message::serialize_msg(&msg);
        let _ = p2p.send_bytes(target_node_id, &bytes).await;
    }
}
