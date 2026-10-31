//! Виджет подключения к комнате с карточками участников

use eframe::egui;

pub fn draw_room_panel(
    ui: &mut eframe::egui::Ui,
    _room_id: &mut String,
    connect_ip: &mut String,
    _connect_port: &mut String,
    network: std::sync::Arc<crate::network::NetworkManager>,
    my_id: u64,
    is_host: std::sync::Arc<std::sync::atomic::AtomicBool>,
    room: std::sync::Arc<crate::room::RoomManager>,
    mixer: &crate::mixer::AudioMixer,
    is_transmitting: bool,
) {
    ui.label(egui::RichText::new("ПОДКЛЮЧЕНИЕ К КОМНАТЕ").strong().color(egui::Color32::from_rgb(180, 185, 195)));
    
    // Адаптивная строка подключения во всю ширину
    ui.horizontal(|ui| {
        let btn_width = 90.0;
        let spacing = ui.spacing().item_spacing.x;
        let input_width = (ui.available_width() - btn_width - spacing).max(120.0);

        ui.add(
            egui::TextEdit::singleline(connect_ip)
                .hint_text("Вставьте NodeID")
                .desired_width(input_width)
        );

        let btn = egui::Button::new(egui::RichText::new("🔗 Войти").strong().color(egui::Color32::WHITE))
            .fill(egui::Color32::from_rgb(14, 165, 233)); // Стильный Electric Sky Blue
        if ui.add_sized([btn_width, 24.0], btn).clicked() {
            let raw_addr = connect_ip.trim();
            
            if raw_addr.len() == 64 && raw_addr.chars().all(|c| c.is_ascii_hexdigit()) {
                if let Some(ref p2p) = network.iroh {
                    let p2p_clone = p2p.clone();
                    let target_node_id = raw_addr.to_string();
                    let my_peer_id = my_id;
                    
                    is_host.store(false, std::sync::atomic::Ordering::SeqCst);
                    println!("[СЕТЬ] Переключился в режим КЛИЕНТА (Iroh P2P). Цель: {}", target_node_id);

                    tokio::task::spawn(async move {
                        println!("[IROH] Попытка P2P-подключения к NodeID: {}", target_node_id);
                        match p2p_clone.connect_to_peer(&target_node_id).await {
                            Ok(()) => {
                                println!("[IROH] 🚀 Успешное прямое P2P-подключение к комнате!");
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
                }
            } else {
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
                            for _ in 1..=6 {
                                let msg = crate::network::NetworkMessage::JoinRequest { sender_id: my_peer_id };
                                let _ = net.send_packet(addr, &msg).await;
                                tokio::time::sleep(tokio::time::Duration::from_millis(350)).await;
                                if !room_clone.get_all_peers().is_empty() {
                                    break;
                                }
                            }
                            let hello_msg = crate::network::NetworkMessage::Heartbeat {
                                sender_id: my_peer_id,
                                is_host: false,
                            };
                            let bytes = crate::network::message::serialize_msg(&hello_msg);
                            let _ = net.send_audio_packet(addr, &bytes).await;
                        });
                        is_host.store(false, std::sync::atomic::Ordering::SeqCst);
                    }
                    Err(e) => eprintln!("[ОШИБКА] Неверный адрес: {}", e),
                }
            }
        }
    });

    let peers = room.get_all_peers();
    let host_peer_opt = peers.iter().find(|p| p.is_host);

    if !is_host.load(std::sync::atomic::Ordering::Relaxed) {
        ui.horizontal(|ui| {
            let host_addr_str = host_peer_opt
                .map(|p| p.address.clone())
                .unwrap_or_else(|| "Ожидание хоста...".to_string());
            
            ui.label(egui::RichText::new(format!("👑 Хост: {}", host_addr_str)).color(egui::Color32::from_rgb(200, 205, 215)));
            if ui.button("📋 Скопировать").clicked() {
                ui.output_mut(|o| o.copied_text = host_addr_str);
            }
        });
    }

    ui.add_space(6.0);
    
    // Статус пользователя
    let my_is_host = is_host.load(std::sync::atomic::Ordering::Relaxed);
    if my_is_host {
        ui.colored_label(egui::Color32::from_rgb(255, 215, 0), "👑 Ваш статус: ХОСТ КОМНАТЫ");
    } else if peers.is_empty() {
        ui.colored_label(egui::Color32::from_rgb(255, 165, 0), "⏳ Подключение к комнате...");
    } else {
        ui.colored_label(egui::Color32::from_rgb(34, 197, 94), "🎧 Ваш статус: В ГОЛОСОВОМ КАНАЛЕ");
    }

    ui.add_space(8.0);
    ui.label(egui::RichText::new(format!("УЧАСТНИКИ В ЭФИРЕ ({})", peers.len() + 1)).strong().color(egui::Color32::from_rgb(180, 185, 195)));

    let card_width = ui.available_width();

    // 1. КАРТОЧКА САМОГО СЕБЯ
    let self_border_color = if is_transmitting {
        egui::Color32::from_rgb(34, 197, 94) // Чистый неоновый зеленый при речи
    } else {
        egui::Color32::from_rgb(52, 56, 64)
    };

    egui::Frame::none()
        .fill(egui::Color32::from_rgb(32, 34, 39))
        .rounding(egui::Rounding::same(8.0))
        .stroke(egui::Stroke::new(if is_transmitting { 2.0 } else { 1.0 }, self_border_color))
        .inner_margin(egui::Margin::symmetric(12.0, 10.0))
        .show(ui, |ui| {
            ui.set_width(card_width);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("👤").size(22.0));
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(format!("Вы (ID: {})", my_id)).strong().color(egui::Color32::WHITE));
                        if my_is_host {
                            ui.colored_label(egui::Color32::from_rgb(255, 215, 0), "👑 [ХОСТ]");
                        }
                    });
                    if is_transmitting {
                        ui.colored_label(egui::Color32::from_rgb(34, 197, 94), "● МИКРОФОН В ЭФИРЕ");
                    } else {
                        ui.colored_label(egui::Color32::from_rgb(160, 166, 178), "○ Ожидание...");
                    }
                });
            });
        });

    ui.add_space(4.0);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    // 2. КАРТОЧКИ СОБЕСЕДНИКОВ
    for peer in peers {
        let is_alive = now.saturating_sub(peer.last_heartbeat) < 3000;
        let card_border = if is_alive {
            egui::Color32::from_rgb(52, 56, 64)
        } else {
            egui::Color32::from_rgb(240, 70, 70)
        };

        egui::Frame::none()
            .fill(egui::Color32::from_rgb(32, 34, 39))
            .rounding(egui::Rounding::same(8.0))
            .stroke(egui::Stroke::new(1.0, card_border))
            .inner_margin(egui::Margin::symmetric(12.0, 10.0))
            .show(ui, |ui| {
                ui.set_width(card_width);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("👤").size(22.0));
                    
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            let (dot_color, dot_text) = if is_alive {
                                (egui::Color32::from_rgb(34, 197, 94), "●")
                            } else {
                                (egui::Color32::from_rgb(180, 185, 195), "○")
                            };
                            ui.colored_label(dot_color, dot_text);
                            
                            let display_addr = if peer.address.starts_with("Iroh:") && peer.address.len() > 18 {
                                format!("{}..", &peer.address[..16])
                            } else {
                                peer.address.clone()
                            };

                            ui.label(egui::RichText::new(format!("ID: {} ({})", peer.id, display_addr)).strong().color(egui::Color32::WHITE));
                            if peer.is_host {
                                ui.colored_label(egui::Color32::from_rgb(255, 215, 0), "👑 [ХОСТ]");
                            }
                        });

                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("Громкость:").size(12.0).color(egui::Color32::from_rgb(180, 185, 195)));
                            let mut p_vol = mixer.get_peer_volume(peer.id);
                            if ui.add(egui::Slider::new(&mut p_vol, 0..=300).suffix("%").show_value(true)).changed() {
                                mixer.set_peer_volume(peer.id, p_vol);
                            }
                        });
                    });
                });
            });

        ui.add_space(4.0);
    }
}