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
        let target_addr = format!("{}:{}", connect_ip, connect_port);
        if let Ok(addr) = target_addr.parse::<std::net::SocketAddr>() {
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
        }
    }
}