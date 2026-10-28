//! Виджет подключения к комнате
use eframe::egui;

/// Виджет для ввода ID комнаты и IP-адреса для подключения
pub fn draw_room_panel(
    ui: &mut eframe::egui::Ui,
    room_id: &mut String,
    connect_ip: &mut String,
    connect_port: &mut String,
    network: std::sync::Arc<crate::network::NetworkManager>,
    my_id: u64,
    is_host: std::sync::Arc<std::sync::atomic::AtomicBool>,
    room: std::sync::Arc<crate::room::RoomManager>,
) {
    ui.label("Подключение к комнате (введите NodeID или IP:PORT):");
    
    ui.horizontal(|ui| {
        ui.text_edit_singleline(connect_ip);
    });

    // Отображение актуального адреса комнаты для клиентов
    let peers = room.get_all_peers();
    let host_peer_opt = peers.iter().find(|p| p.is_host);

    if !is_host.load(std::sync::atomic::Ordering::Relaxed) {
        ui.horizontal(|ui| {
            let host_addr_str = host_peer_opt
                .map(|p| p.address.clone())
                .unwrap_or_else(|| "Ожидание хоста...".to_string());
            
            ui.label(format!("👑 Хост комнаты: {}", host_addr_str));
            if ui.button("📋 Скопировать").on_hover_text("Скопировать адрес комнаты для друзей").clicked() {
                ui.output_mut(|o| o.copied_text = host_addr_str);
            }
        });
    }
    
    ui.horizontal(|ui| {
        // Кнопка для хоста: пробивает NAT наружу к участнику, сохраняя статус хоста
        if is_host.load(std::sync::atomic::Ordering::Relaxed) {
            if ui.button("🚀 Впустить участника (Пробить NAT)").clicked() {
                let raw_addr = connect_ip.trim();
                let target_addr: Result<std::net::SocketAddr, _> = if raw_addr.contains(':') {
                    raw_addr.parse()
                } else {
                    format!("{}:8080", raw_addr).parse()
                };

                match target_addr {
                    Ok(addr) => {
                        let host_addr = match network.local_addr() {
                            Ok(a) => a,
                            Err(_) => "0.0.0.0:0".parse().unwrap(),
                        };
                        let update_msg = crate::network::message::NetworkMessage::PeerListUpdate {
                            peers: vec![(my_id, host_addr)],
                        };
                        let serialized = crate::network::message::serialize_msg(&update_msg);
                        let net = network.clone();
                        
                        // Добавляем участника в список, чтобы фоновый heartbeat сразу начал пробив
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64;
                        let new_peer = crate::types::Peer {
                            id: 0,
                            address: addr.to_string(),
                            is_host: false,
                            last_heartbeat: now,
                        };
                        room.update_peer(new_peer);

                        tokio::task::spawn(async move {
                            for _ in 0..3 {
                                let _ = net.send_audio_packet(addr, &serialized).await;
                                tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                            }
                            println!("[СЕТЬ] Хост отправил встречный пробив NAT на: {}", addr);
                        });
                    }
                    Err(e) => {
                        eprintln!("[ОШИБКА] Не удалось разобрать адрес \"{}\": {}", raw_addr, e);
                    }
                }
            }
        }

        // Кнопка для подключения клиентом к другому серверу
        if ui.button("🔗 Войти в комнату").clicked() {
            let raw_addr = connect_ip.trim();
            
            // Проверяем, является ли введенный адрес NodeID Iroh (длинной 64 символа)
            if raw_addr.len() == 64 && raw_addr.chars().all(|c| c.is_ascii_hexdigit()) {
                // Это NodeID - подключаемся через Iroh P2P
                if let Some(ref p2p) = network.iroh {
                    let p2p_clone = p2p.clone();
                    let target_node_id = raw_addr.to_string();
                    let my_peer_id = my_id;
                    
                    // Переключаем статус клиента
                    is_host.store(false, std::sync::atomic::Ordering::SeqCst);
                    println!("[СЕТЬ] Переключился в режим КЛИЕНТА (Iroh P2P). Цель: {}", target_node_id);

                    tokio::task::spawn(async move {
                        println!("[IROH] Попытка P2P-подключения к NodeID: {}", target_node_id);
                        match p2p_clone.connect_to_peer(&target_node_id).await {
                            Ok(()) => {
                                println!("[IROH] 🚀 Успешное прямое P2P-подключение к комнате!");
                                // Отправляем приветственный Heartbeat для мгновенного появления в списке
                                let hello_msg = crate::network::NetworkMessage::Heartbeat {
                                    sender_id: my_peer_id,
                                    is_host: false,
                                };
                                let bytes = crate::network::message::serialize_msg(&hello_msg);
                                p2p_clone.broadcast_bytes(&bytes).await;
                            }
                            Err(e) => eprintln!("[IROH] Ошибка P2P-подключения по NodeID: {}", e),
                        }
                    });
                } else {
                    eprintln!("[ОШИБКА] Iroh не инициализирован, невозможно подключиться по NodeID");
                }
            } else {
                // Это IP:PORT - обычное подключение
                let target_addr: Result<std::net::SocketAddr, _> = if raw_addr.contains(':') {
                    raw_addr.parse()
                } else {
                    format!("{}:8080", raw_addr).parse()
                };

                match target_addr {
                    Ok(addr) => {
                        let net = network.clone();
                        let room_clone = room.clone();
                        let my_peer_id = my_id;
                        tokio::task::spawn(async move {
                            println!("[СЕТЬ] Запуск серии запросов на подключение к {}", addr);
                            for attempt in 1..=6 {
                                let msg = crate::network::NetworkMessage::JoinRequest {
                                    sender_id: my_peer_id,
                                };
                                let _ = net.send_packet(addr, &msg).await;
                                tokio::time::sleep(tokio::time::Duration::from_millis(350)).await;
                                if !room_clone.get_all_peers().is_empty() {
                                    println!("[СЕТЬ] Соединение установлено с попытки #{}!", attempt);
                                    break;
                                }
                            }
                            
                            // Отправляем приветственный Heartbeat для мгновенного появления в списке
                            let hello_msg = crate::network::NetworkMessage::Heartbeat {
                                sender_id: my_peer_id,
                                is_host: false,
                            };
                            let bytes = crate::network::message::serialize_msg(&hello_msg);
                            // Отправляем через UDP
                            let _ = net.send_audio_packet(addr, &bytes).await;
                        });
                        // Переключаем статус клиента
                        is_host.store(false, std::sync::atomic::Ordering::SeqCst);
                        println!("[СЕТЬ] Переключился в режим КЛИЕНТА. Цель: {}", addr);
                    }
                    Err(e) => {
                        eprintln!("[ОШИБКА] Неверный адрес \"{}\": {}", raw_addr, e);
                    }
                }
            }
        }
    });
    
    ui.separator();
    
    // Отображаем текущую роль пользователя
    if is_host.load(std::sync::atomic::Ordering::Relaxed) {
        ui.colored_label(egui::Color32::from_rgb(255, 215, 0), "👑 Ваш статус: ХОСТ (Ожидание участников)");
    } else {
        let peers = room.get_all_peers();
        if peers.is_empty() {
            ui.colored_label(egui::Color32::from_rgb(255, 165, 0), "⏳ Подключение к хосту (ожидание ответа сети)...");
        } else {
            ui.colored_label(egui::Color32::from_rgb(100, 200, 100), "🎧 Ваш статус: В СЕТИ (Подключен к комнате)");
        }
    }

    let peers = room.get_all_peers();
    ui.heading(format!("Участники в сети ({})", peers.len() + 1));
    
    // Отображаем самого себя
    ui.horizontal(|ui| {
        ui.label(format!("• Вы (ID: {})", my_id));
        if is_host.load(std::sync::atomic::Ordering::Relaxed) {
            ui.colored_label(egui::Color32::from_rgb(255, 215, 0), "[ХОСТ]");
        }
    });

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    // Отображаем остальных пиров
    for peer in peers {
        ui.horizontal(|ui| {
            let is_alive = now.saturating_sub(peer.last_heartbeat) < 3000;
            let (dot_color, dot_char) = if is_alive {
                (egui::Color32::from_rgb(0, 255, 120), "●")
            } else {
                (egui::Color32::from_rgb(255, 140, 0), "○")
            };
            ui.colored_label(dot_color, dot_char);
            ui.label(format!("ID: {} ({})", peer.id, peer.address));
            if peer.is_host {
                ui.colored_label(egui::Color32::from_rgb(255, 215, 0), "[ХОСТ]");
            }
        });
    }
}