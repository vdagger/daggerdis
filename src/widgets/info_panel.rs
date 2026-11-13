//! Верхняя панель: логотип, ID и вход в настройки

use crate::gui::theme as t;
use eframe::egui;

fn logo_texture(ui: &mut egui::Ui) -> egui::TextureHandle {
    if let Some(tex) = ui
        .ctx()
        .data_mut(|d| d.get_temp::<egui::TextureHandle>(egui::Id::new("app_logo_texture")))
    {
        return tex;
    }

    let logo_bytes = include_bytes!("../../assets/logo.png");
    let icon = eframe::icon_data::from_png_bytes(logo_bytes).expect("Failed to load logo.png");
    let color_image =
        egui::ColorImage::from_rgba_unmultiplied([icon.width as usize, icon.height as usize], &icon.rgba);
    let tex = ui
        .ctx()
        .load_texture("app_logo", color_image, Default::default());
    ui.ctx()
        .data_mut(|d| d.insert_temp(egui::Id::new("app_logo_texture"), tex.clone()));
    tex
}

/// Кнопка-иконка с ползунками (настройки)
fn settings_button(ui: &mut egui::Ui, open: bool) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(32.0, 32.0), egui::Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);

    let bg = if open {
        t::with_alpha(t::BRAND, 0.18)
    } else if response.hovered() {
        t::SURFACE_HI
    } else {
        t::SURFACE
    };
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, egui::Rounding::same(9.0), bg);
    painter.rect_stroke(
        rect,
        egui::Rounding::same(9.0),
        egui::Stroke::new(1.0_f32, if open { t::with_alpha(t::BRAND, 0.5) } else { t::STROKE }),
    );

    let color = if open {
        t::BRAND
    } else if response.hovered() {
        t::TEXT
    } else {
        t::TEXT_DIM
    };

    let cx = rect.center().x;
    let cy = rect.center().y;
    for (dy, knob_t) in [(-5.0_f32, 0.35_f32), (5.0, 0.65)] {
        let y = cy + dy;
        painter.line_segment(
            [egui::pos2(cx - 8.0, y), egui::pos2(cx + 8.0, y)],
            egui::Stroke::new(1.5_f32, color),
        );
        painter.circle_filled(
            egui::pos2(cx - 8.0 + 16.0 * knob_t, y),
            2.6,
            color,
        );
    }

    response
}

pub fn draw_top_bar(ui: &mut egui::Ui, my_id: u64, settings_open: &mut bool) {
    let texture = logo_texture(ui);

    ui.horizontal(|ui| {
        ui.add(egui::Image::new(&texture).max_height(26.0));

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let btn = settings_button(ui, *settings_open);
            if btn.clicked() {
                *settings_open = !*settings_open;
            }
            btn.on_hover_text("Настройки");

            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(format!("{}", my_id))
                    .monospace()
                    .size(10.0)
                    .color(t::TEXT_MUTED),
            );
        });
    });
}
