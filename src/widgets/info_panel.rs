//! Панель информации о подключении

use eframe::egui;

/// Отрисовка панели информации о подключении
pub fn draw_info_panel(ui: &mut eframe::egui::Ui, my_id: u64, my_external_ip: &str) {
    ui.label(format!("Мой ID: #{}", my_id));
    ui.label(format!("Внешний IP: {}", my_external_ip));
}