//! Фоновый сервис приема и ретрансляции пакетов через Iroh P2P

use crate::calls::CallManager;
use crate::chat::ChatLog;
use crate::friends::FriendsManager;
use crate::network::p2p_iroh::IrohP2pManager;
use crate::network::NetworkMessage;
use crate::room::RoomManager;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc::Sender;

/// Проверяет, что `sender_id` из пакета — это наш хеш от аутентичного NodeID
/// QUIC-соединения. Любой другой sender_id = попытка выдать себя за другого.
fn sender_id_verified(remote_node_id: &str, sender_id: u64) -> bool {
    match crate::identity::my_id_from_node_id(remote_node_id) {
        Some(id) => id == sender_id,
        None => false,
    }
}

/// Запускает фоновую таску приема аудио и Heartbeat через Iroh, включая ретрансляцию (SFU) на хосте
pub fn spawn_iroh_receiver(
    rt: &tokio::runtime::Runtime,
    iroh_p2p: Option<Arc<IrohP2pManager>>,
    room: Arc<RoomManager>,
    playback_tx: Sender<(u64, u32, Vec<u8>)>,
    is_host: Arc<AtomicBool>,
    friends: Arc<FriendsManager>,
    chat_log: Arc<ChatLog>,
    room_chat: Arc<crate::chat::RoomChat>,
    calls: Arc<CallManager>,
    presence: Arc<crate::presence::PresenceTracker>,
    video: Arc<crate::network::video_directory::VideoDirectory>,
) {
    if let Some(p2p) = iroh_p2p {
        let p2p_recv = p2p.clone();
        let playback_tx_iroh = playback_tx.clone();
        let room_iroh = room.clone();
        let is_host_iroh = is_host.clone();
        let friends_iroh = friends.clone();
        let chat_log_iroh = chat_log.clone();
        let room_chat_iroh = room_chat.clone();
        let calls_iroh = calls.clone();
        let presence_iroh = presence.clone();
        let video_iroh = video.clone();
        let my_id = crate::identity::my_id_from_node_id(&p2p.my_node_id).unwrap_or(0);

        rt.spawn(async move {
            println!("[IROH] Слушатель входящих P2P пакетов активен.");
            while let Some(pkt) = p2p_recv.recv_packet().await {
                // `pkt.sender_id` — это аутентичный NodeID удалённого узла (из QUIC/TLS).
                // Хеш от него даёт ожидаемый sender_id; всё остальное отбрасываем.
                if let Some(msg) = crate::network::message::deserialize_msg(&pkt.data) {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;

                    // Любой пакет от узла — доказательство, что он в сети.
                    // Отмечаем именно `pkt.sender_id`: это NodeID соединения
                    // из QUIC/TLS, а не поле пакета (его может подделать
                    // релейный IrohSync с чужим node_id).
                    presence_iroh.touch(&pkt.sender_id);

                    match msg {
                        NetworkMessage::IrohSync {
                            sender_id,
                            node_id,
                            is_host: peer_is_host,
                            hops,
                            video_gen,
                        } => {
                            if hops > 0 {
                                // Релей: хост пересылает IrohSync тем, с кем этот
                                // узел ещё не соединился напрямую. Подлинность
                                // соединения тут не проверить, поэтому пускаем
                                // только один релей и только от нашего хоста.
                                let id_ok = crate::identity::my_id_from_node_id(&node_id)
                                    == Some(sender_id);
                                if hops > 1 || !id_ok || node_id == p2p_recv.my_node_id {
                                    eprintln!(
                                        "[БЕЗОПАСНОСТЬ] Релейный IrohSync от {} отброшен",
                                        pkt.sender_id
                                    );
                                    continue;
                                }
                                let relayed_by_host = room_iroh
                                    .get_all_peers()
                                    .iter()
                                    .any(|p| p.address == pkt.sender_id && p.is_host);
                                if !relayed_by_host {
                                    eprintln!(
                                        "[БЕЗОПАСНОСТЬ] IrohSync по релею не от хоста ({}) отброшен",
                                        pkt.sender_id
                                    );
                                    continue;
                                }
                            } else if !sender_id_verified(&pkt.sender_id, sender_id)
                                || node_id != pkt.sender_id
                            {
                                // Узел обязан назвать в пакете тот же NodeID, что у
                                // соединения, а sender_id должен быть хешем этого NodeID.
                                eprintln!(
                                    "[БЕЗОПАСНОСТЬ] IrohSync с чужим sender_id/NodeID от {} отброшен",
                                    pkt.sender_id
                                );
                                continue;
                            }

                            // Звонящий, которого ещё не приняли, в комнату не попадает:
                            // IrohSync рассылается раз в секунду и иначе добавлял бы
                            // его в участники в обход согласия на звонок.
                            // Пользователь вышел из эфира сам — не возвращаем его
                            // туда авто-синхронизацией.
                            if calls_iroh.is_blocked(&pkt.sender_id) {
                                continue;
                            }
                            if room_iroh.is_left() {
                                // Пока стоит «вышли из эфира», комната не
                                // принимает синхронизацию — иначе узел вернулся
                                // бы в неё сам. Если это видно в журнале после
                                // «Войти по приглашению», значит флаг не снят.
                                eprintln!(
                                    "[КОМНАТА] IrohSync от {} отброшен: мы вне эфира",
                                    pkt.sender_id
                                );
                                continue;
                            }

                            // Анонс трансляции — только после проверок подлинности:
                            // подделанный sender_id не должен подсунуть зрителю
                            // чужой поток. Номер 0 снимает анонс.
                            video_iroh.observe(&node_id, video_gen);

                            // 1. Сохраняем пира с ПОЛНЫМ 64-символьным NodeID
                            room_iroh.update_peer(crate::types::Peer {
                                id: sender_id,
                                address: node_id.clone(),
                                is_host: peer_is_host,
                                last_heartbeat: now,
                            });

                            // 2. Full-Mesh авто-коннект: если мы еще не соединены напрямую с этим узлом
                            if node_id != p2p_recv.my_node_id {
                                let p2p_connect = p2p_recv.clone();
                                let target_id = node_id.clone();
                                let friends_resend = friends_iroh.clone();
                                tokio::spawn(async move {
                                    let connected = p2p_connect.get_connected_peers().await;
                                    if !connected.contains(&target_id) {
                                        println!(
                                            "[IROH] Авто-связывание Full-Mesh с NodeID: {}",
                                            target_id
                                        );
                                        let _ = p2p_connect.connect_to_peer(&target_id).await;
                                    }
                                    // Повторно шлём не доставленные заявки после соединения
                                    crate::friends::resend_pending_to(
                                        friends_resend.as_ref(),
                                        &p2p_connect,
                                        &target_id,
                                    )
                                    .await;
                                });
                            }

                            // 3. Релеинг: если мы Хост, пересылаем IrohSync остальным
                            // участникам — с увеличенным hops, чтобы пакет не ходил
                            // по кругу и дальше одного релея не уходил.
                            if is_host_iroh.load(Ordering::Relaxed) && hops < 2 {
                                let relay = NetworkMessage::IrohSync {
                                    sender_id,
                                    node_id: node_id.clone(),
                                    is_host: peer_is_host,
                                    hops: hops + 1,
                                    // без поколения зритель за релеем не узнает
                                    // о трансляции — пакет собирается заново
                                    video_gen,
                                };
                                let bytes = crate::network::message::serialize_msg(&relay);
                                p2p_recv
                                    .broadcast_bytes_except(&pkt.sender_id, &bytes)
                                    .await;
                            }
                        }
                        NetworkMessage::HostMigration {
                            sender_id,
                            new_host_id,
                        } => {
                            // Объявить нового хоста может только он сам: иначе
                            // любой участник перехватывал бы роль хоста одним
                            // датаграмом.
                            if !sender_id_verified(&pkt.sender_id, sender_id)
                                || sender_id != new_host_id
                            {
                                eprintln!(
                                    "[БЕЗОПАСНОСТЬ] HostMigration от {} (заявляет хостом {}) отброшен",
                                    pkt.sender_id, new_host_id
                                );
                                continue;
                            }
                            println!(
                                "[IROH] 👑 Получено уведомление о смене хоста. Новый лидер ID: {}",
                                new_host_id
                            );
                            room_iroh.set_host(new_host_id);
                            if new_host_id == room_iroh.get_my_id() {
                                is_host_iroh.store(true, Ordering::SeqCst);
                            } else {
                                is_host_iroh.store(false, Ordering::SeqCst);
                            }
                        }
                        NetworkMessage::AudioFrame {
                            sender_id,
                            seq,
                            opus_data,
                        } => {
                            if !sender_id_verified(&pkt.sender_id, sender_id) {
                                continue;
                            }
                            if calls_iroh.is_blocked(&pkt.sender_id) || room_iroh.is_left() {
                                continue;
                            }
                            let is_peer_host = room_iroh
                                .get_all_peers()
                                .iter()
                                .find(|p| p.id == sender_id)
                                .map(|p| p.is_host)
                                .unwrap_or(false);

                            room_iroh.update_peer(crate::types::Peer {
                                id: sender_id,
                                address: pkt.sender_id.clone(),
                                is_host: is_peer_host,
                                last_heartbeat: now,
                            });

                            let _ = playback_tx_iroh.try_send((sender_id, seq, opus_data));
                            // Ретрансляция голоса удалена, чтобы исключить задвоение сэмплов и замедление звука
                        }
                        NetworkMessage::Heartbeat {
                            sender_id,
                            is_host: peer_is_host,
                        } => {
                            if !sender_id_verified(&pkt.sender_id, sender_id) {
                                continue;
                            }
                            if calls_iroh.is_blocked(&pkt.sender_id) || room_iroh.is_left() {
                                continue;
                            }
                            room_iroh.update_peer(crate::types::Peer {
                                id: sender_id,
                                address: pkt.sender_id.clone(),
                                is_host: peer_is_host,
                                last_heartbeat: now,
                            });

                            if is_host_iroh.load(Ordering::Relaxed) {
                                p2p_recv
                                    .broadcast_bytes_except(&pkt.sender_id, &pkt.data)
                                    .await;
                            }
                        }
                        NetworkMessage::FriendRequest {
                            sender_id, nick, ..
                        } => {
                            if !sender_id_verified(&pkt.sender_id, sender_id) {
                                eprintln!(
                                    "[БЕЗОПАСНОСТЬ] FriendRequest с чужим sender_id от {} отброшен",
                                    pkt.sender_id
                                );
                                continue;
                            }
                            friends_iroh.add_incoming(crate::friends::FriendRequest {
                                sender_id,
                                node_id: pkt.sender_id.clone(),
                                nick: nick.clone(),
                            });
                            println!(
                                "[ДРУЗЬЯ] Входящая заявка от {} ({}).",
                                nick, pkt.sender_id
                            );
                        }
                        NetworkMessage::FriendAccept {
                            sender_id,
                            node_id,
                            nick,
                        } => {
                            if !sender_id_verified(&pkt.sender_id, sender_id)
                                || node_id != pkt.sender_id
                            {
                                eprintln!(
                                    "[БЕЗОПАСНОСТЬ] FriendAccept с чужим sender_id/NodeID от {} отброшен",
                                    pkt.sender_id
                                );
                                continue;
                            }
                            friends_iroh.resolve_outgoing(&node_id, &nick);
                            println!("[ДРУЗЬЯ] {} принял заявку.", nick);
                        }
                        NetworkMessage::FriendRemove { sender_id, .. } => {
                            if !sender_id_verified(&pkt.sender_id, sender_id) {
                                continue;
                            }
                            friends_iroh.remove_friend(&pkt.sender_id);
                            println!(
                                "[ДРУЗЬЯ] {} удалил вас из друзей.",
                                pkt.sender_id
                            );
                        }
                        NetworkMessage::ChatMessage { sender_id, text } => {
                            if !sender_id_verified(&pkt.sender_id, sender_id) {
                                continue;
                            }
                            if !friends_iroh.is_friend(&pkt.sender_id) {
                                eprintln!(
                                    "[ЧАТ] Сообщение от незнакомца {} отброшено.",
                                    pkt.sender_id
                                );
                                continue;
                            }
                            chat_log_iroh.push(&pkt.sender_id, crate::chat::ChatMessage {
                                from_me: false,
                                text,
                                at: now,
                            });
                        }
                        NetworkMessage::RoomChat {
                            sender_id,
                            room_id,
                            nick,
                            text,
                        } => {
                            if !sender_id_verified(&pkt.sender_id, sender_id) {
                                continue;
                            }
                            // Вне эфира комната нас не касается: сообщение
                            // прилетело бы из разговора, которого мы не слышим.
                            if room_iroh.is_left() {
                                eprintln!(
                                    "[ЧАТ] Сообщение в комнату от {} отброшено: мы вне эфира",
                                    pkt.sender_id
                                );
                                continue;
                            }
                            // Писать в эфир может только его участник: чужой не
                            // должен вклиниваться в комнату без приглашения.
                            if !room_iroh
                                .get_all_addresses()
                                .iter()
                                .any(|a| a == &pkt.sender_id)
                            {
                                eprintln!(
                                    "[ЧАТ] Сообщение в комнату от {} отброшено: его нет в комнате",
                                    pkt.sender_id
                                );
                                continue;
                            }
                            // 0 — общий эфир без созвона; иначе номера комнат
                            // должны совпадать.
                            let my_room = calls_iroh.room_id().unwrap_or(0);
                            if room_id != 0 && my_room != 0 && room_id != my_room {
                                continue;
                            }
                            let text = text.trim().to_string();
                            if text.is_empty() {
                                continue;
                            }
                            println!(
                                "[ЧАТ] {} → в комнату: {} симв.",
                                nick,
                                text.chars().count()
                            );
                            room_chat_iroh.push(
                                room_id,
                                crate::chat::RoomMessage {
                                    from_me: false,
                                    sender_id,
                                    nick,
                                    text,
                                    at: now,
                                },
                            );
                        }
                        NetworkMessage::CallInvite {
                            sender_id,
                            room_id,
                            nick,
                        } => {
                            if !sender_id_verified(&pkt.sender_id, sender_id) {
                                continue;
                            }
                            // Звонить могут только друзья: чужой не должен
                            // заставлять окно звонить и тянуть к себе комнату.
                            if !friends_iroh.is_friend(&pkt.sender_id) {
                                eprintln!(
                                    "[ЗВОНКИ] Приглашение от незнакомца {} отброшено.",
                                    pkt.sender_id
                                );
                                continue;
                            }
                            println!(
                                "[ЗВОНКИ] Входящий звонок от {} ({})",
                                nick, pkt.sender_id
                            );
                            calls_iroh.on_invite(&pkt.sender_id, &nick, room_id);
                        }
                        NetworkMessage::CallAccept {
                            sender_id,
                            room_id,
                        } => {
                            if !sender_id_verified(&pkt.sender_id, sender_id) {
                                continue;
                            }
                            calls_iroh.on_accept(&pkt.sender_id, room_id);
                        }
                        NetworkMessage::CallDecline {
                            sender_id,
                            room_id,
                            reason,
                        } => {
                            if !sender_id_verified(&pkt.sender_id, sender_id) {
                                continue;
                            }
                            calls_iroh.on_decline(&pkt.sender_id, room_id, reason);
                        }
                        NetworkMessage::CallCancel {
                            sender_id,
                            room_id,
                        } => {
                            if !sender_id_verified(&pkt.sender_id, sender_id) {
                                continue;
                            }
                            calls_iroh.on_cancel(&pkt.sender_id, room_id);
                        }
                        NetworkMessage::CallEnd {
                            sender_id,
                            room_id,
                        } => {
                            if !sender_id_verified(&pkt.sender_id, sender_id) {
                                continue;
                            }
                            calls_iroh.on_end(&pkt.sender_id, room_id);
                            // «Выйти из эфира» — это же и уход из mesh: если
                            // оставить запись в списке, остальные видят человека
                            // «в комнате», а пригласить его уже нельзя —
                            // start_outgoing отвечает «он уже в комнате».
                            room.remove_peer(sender_id);
                        }
                        NetworkMessage::RoomMembers {
                            sender_id,
                            room_id,
                            members,
                        } => {
                            if !sender_id_verified(&pkt.sender_id, sender_id) {
                                continue;
                            }
                            // Номер комнаты известен только приглашённым: список
                            // от чужого (или от другой комнаты) не применяем.
                            if calls_iroh.room_id() != Some(room_id) {
                                eprintln!(
                                    "[КОМНАТА] Список комнаты от {} не для нашей комнаты — отброшен",
                                    pkt.sender_id
                                );
                                continue;
                            }
                            println!(
                                "[КОМНАТА] Список комнаты от {}: {} участник(ов)",
                                pkt.sender_id,
                                members.len()
                            );
                            // Дотягиваемся до каждого участника: без этого новый
                            // в комнате остался бы соединён только с пригласившим
                            // и не слышал бы остальных.
                            for node in members {
                                if node == p2p_recv.my_node_id {
                                    continue;
                                }
                                let p2p = p2p_recv.clone();
                                tokio::spawn(async move {
                                    let connected = p2p.get_connected_peers().await;
                                    if !connected.contains(&node) {
                                        let _ = p2p.connect_to_peer(&node).await;
                                    }
                                });
                            }
                        }
                        NetworkMessage::PresencePing { sender_id } => {
                            if !sender_id_verified(&pkt.sender_id, sender_id) {
                                continue;
                            }
                            presence_iroh.touch(&pkt.sender_id);
                            let bytes = crate::network::message::serialize_msg(
                                &NetworkMessage::PresencePong { sender_id: my_id },
                            );
                            let p2p = p2p_recv.clone();
                            let target = pkt.sender_id.clone();
                            tokio::spawn(async move {
                                let _ = p2p.send_bytes(&target, &bytes).await;
                            });
                        }
                        NetworkMessage::PresencePong { sender_id } => {
                            if !sender_id_verified(&pkt.sender_id, sender_id) {
                                continue;
                            }
                            presence_iroh.touch(&pkt.sender_id);
                        }
                    }
                }
            }
        });
    }
}




