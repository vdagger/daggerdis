//! Центральный блок эфира: свой аватар, состояние и осциллограмма

use crate::gui::theme as t;
use eframe::egui;

pub fn draw_hero(
    ui: &mut egui::Ui,
    level: f32,
    speaking: bool,
    is_host: bool,
    wave: &[f32],
    peer_count: usize,
) {
    let level = level.clamp(0.0, 1.0);
    let time = ui.input(|i| i.time) as f32;

    ui.vertical_centered(|ui| {
        let status = if peer_count == 0 {
            "ЭФИР ТИХ"
        } else {
            "В ЭФИРЕ"
        };
        ui.label(
            egui::RichText::new(format!("{} · {}", status, peer_count + 1))
                .size(10.0)
                .strong()
                .color(t::TEXT_MUTED),
        );

        ui.add_space(6.0);

        let (rect, _) = ui.allocate_exact_size(egui::vec2(100.0, 100.0), egui::Sense::hover());
        let center = rect.center();
        // Свечение рисуем без обрезки по выделенному прямоугольнику — оно должно «дышать» наружу
        let glow = ui.painter();

        let breath = 0.5 + 0.5 * (time * 0.9).sin();
        let pulse = if speaking {
            0.5 + 0.5 * (time * 4.0).sin()
        } else {
            0.0
        };

        t::radial_ring(
            glow,
            center,
            36.0,
            138.0,
            t::with_alpha(t::BRAND, 0.020 + 0.014 * breath + 0.070 * pulse),
            t::with_alpha(t::BRAND, 0.0),
        );
        t::radial_ring(
            glow,
            center,
            30.0,
            80.0,
            t::with_alpha(t::BRAND, 0.014 + 0.012 * breath + 0.050 * pulse),
            t::with_alpha(t::BRAND, 0.0),
        );

        // Кольца, расходящиеся от аватара, пока идёт передача
        if speaking {
            for i in 0..3 {
                let phase = (time * 0.55 + i as f32 / 3.0) % 1.0;
                glow.circle_stroke(
                    center,
                    42.0 + phase * 62.0,
                    egui::Stroke::new(
                        1.0_f32,
                        t::with_alpha(t::LIVE, (1.0 - phase).powi(2) * 0.22),
                    ),
                );
            }
        }

        t::sweep_arc(glow, center, 47.0, time, t::BRAND, 0.45);

        let painter = ui.painter_at(rect);

        t::radial_fill(
            &painter,
            center,
            31.0,
            egui::Color32::from_rgb(48, 52, 62),
            egui::Color32::from_rgb(23, 25, 31),
        );

        painter.circle_stroke(
            center,
            40.0,
            egui::Stroke::new(2.0_f32, t::with_alpha(t::TEXT_MUTED, 0.38)),
        );
        let arc_color = if level > 0.9 { t::REC } else { t::LIVE };
        t::level_arc(&painter, center, 40.0, level, arc_color);

        let dot = 6.0 + 14.0 * level;
        if speaking {
            painter.circle_filled(center, dot * 2.0, t::with_alpha(t::LIVE, 0.12));
        }
        painter.circle_filled(
            center,
            dot,
            if speaking { t::LIVE } else { t::TEXT_DIM },
        );

        ui.add_space(4.0);

        ui.label(
            egui::RichText::new("Вы")
                .size(17.0)
                .strong()
                .color(t::TEXT),
        );

        ui.add_space(2.0);

        ui.horizontal(|ui| {
            let width = 150.0;
            let offset = (ui.available_width() - width) * 0.5;
            if offset > 0.0 {
                ui.add_space(offset);
            }
            if is_host {
                t::badge(ui, "ХОСТ", t::GOLD);
            }
            if speaking {
                t::badge(ui, "ГОВОРИТ", t::LIVE);
            } else {
                t::badge(ui, "МОЛЧИТ", t::TEXT_MUTED);
            }
        });

        ui.add_space(8.0);

        let (wave_rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 30.0), egui::Sense::hover());
        let painter = ui.painter_at(wave_rect);
        t::waveform(
            &painter,
            wave_rect,
            wave,
            if speaking { t::LIVE } else { t::TEXT_MUTED },
        );
    });
}
