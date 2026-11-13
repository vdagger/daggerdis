use crate::gui::theme as t;
use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Главный орган управления: пилюля передачи и тумблер постоянного эфира
pub fn draw_ptt_button(
    ui: &mut egui::Ui,
    ptt_flag: &Arc<AtomicBool>,
    continuous_mode: &mut bool,
    mixer: &crate::mixer::AudioMixer,
) {
    let transmitting = ptt_flag.load(Ordering::Relaxed);
    let level = mixer.input1_peak().max(mixer.input2_peak()).clamp(0.0, 1.0);

    let (fill, border, title) = if *continuous_mode {
        (t::with_alpha(t::LIVE, 0.07), t::LIVE, "ЭФИР ОТКРЫТ")
    } else if transmitting {
        (t::with_alpha(t::REC, 0.10), t::REC, "ПЕРЕДАЧА")
    } else {
        (t::SURFACE, t::STROKE, "УДЕРЖИВАЙ, ЧТОБЫ ГОВОРИТЬ")
    };

    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 56.0),
        egui::Sense::click_and_drag(),
    );
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);

    if ui.is_rect_visible(rect) {
        let painter = ui.painter_at(rect);
        let rounding = egui::Rounding::same(16.0);

        if transmitting || *continuous_mode {
            t::glow_rect(&painter, rect, rounding, border, 0.55);
        } else if response.hovered() {
            t::glow_rect(&painter, rect, rounding, t::BRAND, 0.30);
        }

        painter.rect_filled(rect, rounding, fill);
        painter.rect_stroke(
            rect,
            rounding,
            egui::Stroke::new(
                if transmitting || *continuous_mode { 1.5_f32 } else { 1.0_f32 },
                border,
            ),
        );

        // Индикатор уровня внутри пилюли
        let bar = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 18.0, rect.bottom() - 12.0),
            egui::vec2(rect.width() - 36.0, 3.0),
        );
        t::vu_meter(&painter, bar, level);

        let text_color = if *continuous_mode {
            t::LIVE
        } else if transmitting {
            egui::Color32::WHITE
        } else if response.hovered() {
            t::TEXT
        } else {
            t::TEXT_DIM
        };

        painter.text(
            egui::pos2(rect.center().x, rect.center().y - 6.0),
            egui::Align2::CENTER_CENTER,
            title,
            egui::FontId::proportional(13.5),
            text_color,
        );

        if transmitting {
            let accent = if *continuous_mode { t::LIVE } else { t::REC };
            let pulse = 0.5 + 0.5 * (ui.input(|i| i.time) * 5.0).sin() as f32;
            painter.circle_filled(
                egui::pos2(rect.left() + 24.0, rect.center().y - 6.0),
                4.0,
                t::with_alpha(accent, 0.35 + 0.65 * pulse),
            );
        }
    }

    if *continuous_mode {
        crate::keybind::set_screen_ptt(false);
    } else if response.is_pointer_button_down_on() {
        crate::keybind::set_screen_ptt(true);
    } else if !ui.input(|i| i.pointer.any_down()) {
        crate::keybind::set_screen_ptt(false);
    }

    ui.add_space(8.0);

    ui.horizontal(|ui| {
        let width = 168.0;
        let offset = (ui.available_width() - width) * 0.5;
        if offset > 0.0 {
            ui.add_space(offset);
        }

        ui.label(
            egui::RichText::new("Постоянный эфир")
                .size(11.5)
                .color(if *continuous_mode { t::TEXT } else { t::TEXT_MUTED }),
        );
        ui.add_space(4.0);

        if t::switch(ui, continuous_mode).changed() {
            ptt_flag.store(*continuous_mode, Ordering::Relaxed);
        }
    });
}
