//! Панель информации о подключении

use eframe::egui;

/// Отрисовка панели информации о подключении
pub fn draw_info_panel(ui: &mut eframe::egui::Ui, my_id: u64, my_external_ip: &str) {
    ui.label(format!("Ваш ID: {}", my_id));
    
    ui.horizontal(|ui| {
        if my_external_ip.len() > 30 {
            ui.colored_label(egui::Color32::from_rgb(0, 220, 255), "🔑 Ваш ID комнаты (NodeID):");
            ui.monospace(my_external_ip);
            if ui.button("📋").on_hover_text("Скопировать NodeID комнаты").clicked() {
                ui.output_mut(|o| o.copied_text = my_external_ip.to_string());
            }
        } else {
            ui.label(format!("Внешний IP: {}", my_external_ip));
        }
    });
}