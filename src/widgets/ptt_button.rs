use crate::gui::theme as t;
use eframe::egui;
use egui::{Rounding, Stroke};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Пилюля передачи: держи, чтобы говорить
pub fn draw_ptt_button(
    ui: &mut egui::Ui,
    ptt_flag: &Arc<AtomicBool>,
    continuous_mode: &mut bool,
    mixer: &crate::mixer::AudioMixer,
    bind: Option<crate::keybind::PttBind>,
) {
    let transmitting = ptt_flag.load(Ordering::Relaxed);
    let _level = mixer.input1_peak().max(mixer.input2_peak()).clamp(0.0, 1.0);

    let (fill, border, title) = if *continuous_mode {
        (t::with_alpha(t::LIVE, 0.08), t::with_alpha(t::LIVE, 0.55), "ЭФИР ОТКРЫТ")
    } else if transmitting {
        (t::with_alpha(t::REC, 0.10), t::with_alpha(t::REC, 0.75), "ПЕРЕДАЧА")
    } else {
        (t::INNER, t::STROKE, "УДЕРЖИВАЙ, ЧТОБЫ ГОВОРИТЬ")
    };

    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 54.0),
        egui::Sense::click_and_drag(),
    );
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);

    if ui.is_rect_visible(rect) {
        let painter = ui.painter_at(rect);
        let rounding = Rounding::same(14.0);

        if transmitting || *continuous_mode {
            t::glow_rect(&painter, rect, rounding, border, 0.45);
        } else if response.hovered() {
            t::glow_rect(&painter, rect, rounding, t::BRAND, 0.22);
        }

        painter.rect_filled(rect, rounding, fill);
        painter.rect_stroke(
            rect,
            rounding,
            Stroke::new(
                if transmitting || *continuous_mode { 1.5_f32 } else { 1.0_f32 },
                border,
            ),
        );

        // Кружок с микрофоном слева
        let mic_center = egui::pos2(rect.left() + 30.0, rect.center().y - 2.0);
        let accent = if *continuous_mode {
            t::LIVE
        } else if transmitting {
            t::REC
        } else {
            t::BRAND
        };
        painter.circle_filled(mic_center, 15.0, t::with_alpha(accent, 0.12));
        painter.circle_stroke(mic_center, 15.0, Stroke::new(1.0_f32, t::with_alpha(accent, 0.30)));
        let mic = egui::Rect::from_center_size(
            mic_center + egui::vec2(0.0, -1.5),
            egui::vec2(6.0, 10.0),
        );
        painter.rect_stroke(mic, Rounding::same(3.0), Stroke::new(1.4_f32, accent));
        painter.line_segment(
            [
                egui::pos2(mic_center.x, mic.bottom() + 1.0),
                egui::pos2(mic_center.x, mic.bottom() + 4.0),
            ],
            Stroke::new(1.4_f32, accent),
        );

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
            egui::pos2(rect.left() + 56.0, rect.center().y - 7.0),
            egui::Align2::LEFT_CENTER,
            title,
            egui::FontId::proportional(12.5),
            text_color,
        );

        // Подсказка — реально назначенная кнопка, а не «пробел»
        let hint = if *continuous_mode {
            "ВКЛ".to_string()
        } else {
            crate::keybind::shortcut_label(
                bind.unwrap_or_else(|| crate::keybind::CURRENT_VK.load(Ordering::Relaxed)),
            )
        };
        painter.text(
            egui::pos2(rect.right() - 14.0, rect.center().y - 1.0),
            egui::Align2::RIGHT_CENTER,
            hint,
            egui::FontId::monospace(9.0),
            t::TEXT_MUTED,
        );

        if transmitting {
            let pulse = 0.5 + 0.5 * (ui.input(|i| i.time) * 5.0).sin() as f32;
            painter.circle_filled(
                egui::pos2(rect.right() - 66.0, rect.center().y - 7.0),
                3.5,
                t::with_alpha(if *continuous_mode { t::LIVE } else { t::REC }, 0.35 + 0.65 * pulse),
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
}

/// Тумблер постоянного эфира для нижней строки консоли
pub fn draw_continuous_toggle(
    ui: &mut egui::Ui,
    ptt_flag: &Arc<AtomicBool>,
    continuous_mode: &mut bool,
) {
    let mut toggle = false;

    ui.horizontal(|ui| {
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::click());
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        let painter = ui.painter_at(rect);
        let rounding = egui::Rounding::same(4.0);
        painter.rect_filled(
            rect,
            rounding,
            if *continuous_mode {
                t::BRAND_2
            } else {
                t::with_alpha(egui::Color32::WHITE, 0.04)
            },
        );
        painter.rect_stroke(
            rect,
            rounding,
            egui::Stroke::new(
                1.0_f32,
                if *continuous_mode {
                    t::BRAND_2
                } else {
                    t::STROKE
                },
            ),
        );
        if *continuous_mode {
            let c = rect.center();
            let ink = egui::Stroke::new(1.6_f32, egui::Color32::WHITE);
            painter.line_segment(
                [
                    egui::pos2(c.x - 3.2, c.y + 0.2),
                    egui::pos2(c.x - 1.0, c.y + 2.6),
                ],
                ink,
            );
            painter.line_segment(
                [
                    egui::pos2(c.x - 1.0, c.y + 2.6),
                    egui::pos2(c.x + 3.4, c.y - 2.6),
                ],
                ink,
            );
        }
        if resp.clicked() {
            toggle = true;
        }

        ui.add_space(4.0);
        let label = ui.add(
            egui::Label::new(
                egui::RichText::new("Постоянный эфир")
                    .size(10.0)
                    .color(if *continuous_mode { t::TEXT } else { t::TEXT_MUTED }),
            )
            .sense(egui::Sense::click()),
        );
        if label.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            toggle = true;
        }
    });

    if toggle {
        *continuous_mode = !*continuous_mode;
        ptt_flag.store(*continuous_mode, Ordering::Relaxed);
    }
}
