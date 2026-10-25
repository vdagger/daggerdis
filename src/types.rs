use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Peer {
    pub id: u64,
    pub address: String,
    pub is_host: bool,
    pub last_heartbeat: u64,
}