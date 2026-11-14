//! Стабильная личность узла: закреплённый секретный ключ iroh и псевдоним my_id.
//!
//! Каноническая личность — это NodeID (публичный ключ Ed25519) узла iroh.
//! `presets::N0` ключ не сохраняет (он лишь настраивает DNS/релеи), поэтому
//! секретный ключ держим сами в `%APPDATA%\Albus\secret.key` (32 байта), чтобы
//! NodeID не менялся между запусками. Криптографию iroh не трогаем — используем
//! штатные `SecretKey`/`PublicKey`, собственный RNG для ключей не пишем.

use iroh::SecretKey;
use std::path::PathBuf;

pub struct Identity {
    pub secret_key: SecretKey,
    pub node_id: String,
    pub my_id: u64,
}

impl Identity {
    /// Загружает закреплённый ключ или создаёт новый (и сразу сохраняет).
    pub fn load_or_create() -> Self {
        let key = Self::load_secret_key().unwrap_or_else(|| {
            let key = SecretKey::generate();
            Self::save_secret_key(&key);
            key
        });
        Self::from_key(key)
    }

    /// Генерирует новый ключ и записывает его на диск. Действует со следующего
    /// запуска: текущий эндпоинт уже привязан к старому ключу и меняться не может.
    pub fn reset() {
        let key = SecretKey::generate();
        Self::save_secret_key(&key);
        println!(
            "[ЛИЧНОСТЬ] Сгенерирован новый NodeID: {} (нужен перезапуск)",
            key.public()
        );
    }

    fn from_key(key: SecretKey) -> Self {
        let node_id = key.public().to_string();
        let my_id = my_id_from_bytes(key.public().as_bytes());
        Identity {
            secret_key: key,
            node_id,
            my_id,
        }
    }

    fn load_secret_key() -> Option<SecretKey> {
        let path = secret_key_path()?;
        let bytes = std::fs::read(&path).ok()?;
        let arr: [u8; 32] = bytes.as_slice().try_into().ok()?;
        Some(SecretKey::from_bytes(&arr))
    }

    fn save_secret_key(key: &SecretKey) {
        let Some(path) = secret_key_path() else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(&path, key.to_bytes()) {
            eprintln!("[ЛИЧНОСТЬ] Не удалось сохранить секретный ключ: {}", e);
        }
    }
}

/// Стабильный 64-битный псевдоним NodeID (FNV-1a). Это метка для маршрутизации
/// и отображения, к шифрованию отношения не имеет.
pub fn my_id_from_bytes(bytes: &[u8; 32]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = OFFSET;
    for &b in bytes.iter() {
        h ^= b as u64;
        h = h.wrapping_mul(PRIME);
    }
    h
}

/// Восстанавливает псевдоним по строковому NodeID (для проверки sender_id).
pub fn my_id_from_node_id(node_id: &str) -> Option<u64> {
    let endpoint_id: iroh::EndpointId = node_id.parse().ok()?;
    Some(my_id_from_bytes(endpoint_id.as_bytes()))
}

fn secret_key_path() -> Option<PathBuf> {
    crate::config::albus_dir().map(|d| d.join("secret.key"))
}
