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
    ui.label("Подключение к комнате:");
    ui.text_edit_singleline(room_id);
    
    ui.label("Подключение по IP:");
    ui.horizontal(|ui| {
        ui.text_edit_singleline(connect_ip);
        ui.label(":");
        ui.text_edit_singleline(connect_port);
    });
    
    // Возвращаем кнопку на законное место в интерфейс
    if ui.button("Подключиться").clicked() {
        let target_addr = format!("{}:{}", connect_ip.trim(), connect_port.trim());
        match target_addr.parse::<std::net::SocketAddr>() {
            Ok(addr) => {
                let msg = crate::protocol::NetworkMessage {
                    room_id: room_id.clone(),
                    sender_id: my_id,
                    packet: crate::protocol::Packet::JoinRoom,
                };
                let net = network.clone();
                tokio::task::spawn(async move {
                    let _ = net.send_packet(addr, &msg).await;
                    println!("[СЕТЬ] Отправлен запрос JoinRoom на адрес: {}", addr);
                });
                // Переключаем флаг в false при успешном подключении
                is_host.store(false, std::sync::atomic::Ordering::SeqCst);
                println!("[СЕТЬ] Переключился в режим КЛИЕНТА. Подключаемся к хосту: {}", addr);
            }
            Err(e) => {
                eprintln!("[ОШИБКА] Не удалось разобрать адрес {}: {}", target_addr, e);
            }
        }
    }
    
    ui.separator();
    
    // Отображаем текущую роль пользователя
    if is_host.load(std::sync::atomic::Ordering::Relaxed) {
        ui.colored_label(egui::Color32::from_rgb(255, 215, 0), "👑 Ваш статус: ХОСТ (Ждём подключений на порту 8080)");
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
            let status = if now.saturating_sub(peer.last_heartbeat) < 2500 {
                "🟢"
            } else {
                "🟠"
            };
            ui.label(format!("{} ID: {} ({})", status, peer.id, peer.address));
            if peer.is_host {
                ui.colored_label(egui::Color32::from_rgb(255, 215, 0), "[ХОСТ]");
            }
        });
    }
}