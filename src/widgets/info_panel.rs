//! Панель информации о подключении

use eframe::egui;

/// Отрисовка панели информации о подключении
pub fn draw_info_panel(ui: &mut eframe::egui::Ui, my_id: u64, my_external_ip: &str) {
    ui.label(format!("Ваш ID: {}", my_id));
    ui.label(format!("Внешний IP: {}", my_external_ip));
    ui.colored_label(
        eframe::egui::Color32::from_rgb(0, 200, 255),
        format!("📢 Адрес для подключения к вам: {}:8080", my_external_ip)
    );
    ui.separator();
}