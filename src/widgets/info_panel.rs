//! Модуль информационной панели с логотипом

use eframe::egui;

pub fn draw_info_panel(ui: &mut egui::Ui, my_id: u64, my_external_ip: &str) {
    let logo_texture = ui.ctx().data_mut(|d| {
        d.get_temp::<egui::TextureHandle>(egui::Id::new("app_logo_texture"))
    });

    let texture = match logo_texture {
        Some(t) => t,
        None => {
            let logo_bytes = include_bytes!("../../assets/logo.png");
            let icon = eframe::icon_data::from_png_bytes(logo_bytes).expect("Failed to load logo.png");
            let color_image = egui::ColorImage::from_rgba_unmultiplied(
                [icon.width as usize, icon.height as usize],
                &icon.rgba,
            );
            let t = ui.ctx().load_texture("app_logo", color_image, Default::default());
            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("app_logo_texture"), t.clone()));
            t
        }
    };

    // Отрисовка логотипа и ID пользователя с отступом справа
    ui.horizontal(|ui| {
        ui.add(egui::Image::new(&texture).max_height(32.0));
        
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(6.0); // Защитный отступ от правого края
            ui.label(
                egui::RichText::new(format!("ID: {}", my_id))
                    .color(egui::Color32::from_rgb(160, 166, 178))
                    .size(12.0),
            );
        });
    });

    ui.add_space(8.0);

    // Отображение NodeID (компактные 18 символов, чтобы не распирать правый край)
    ui.horizontal(|ui| {
        if my_external_ip.len() > 30 {
            ui.colored_label(egui::Color32::from_rgb(0, 220, 255), "🔑 Ваш ID комнаты:");
            
            let short_display = if my_external_ip.len() > 20 {
                format!("{}..", &my_external_ip[..18])
            } else {
                my_external_ip.to_string()
            };
            ui.monospace(egui::RichText::new(&short_display).color(egui::Color32::WHITE));
            
            if ui.button("📋").on_hover_text("Скопировать полный NodeID комнаты").clicked() {
                ui.output_mut(|o| o.copied_text = my_external_ip.to_string());
            }
        } else {
            ui.label(format!("Внешний IP: {}", my_external_ip));
        }
    });
}