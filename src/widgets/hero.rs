//! Центральный блок эфира: диск рации, состояние и осциллограмма

use crate::gui::members_label;
use crate::gui::theme as t;
use eframe::egui;
use egui::{Pos2, Rounding, Stroke};

/// Диск рации: насечки, дуга уровня, пульс Albus в центре
pub fn draw_dial(
    painter: &egui::Painter,
    center: Pos2,
    radius: f32,
    level: f32,
    speaking: bool,
    time: f32,
) {
    let level = level.clamp(0.0, 1.0);
    let breath = 0.5 + 0.5 * (time * 0.9).sin();
    let pulse = if speaking {
        0.5 + 0.5 * (time * 4.0).sin()
    } else {
        0.0
    };

    // Лёгкий фиолетовый ореол только у диска, без мутного кольца-бублика
    t::glow_circle(
        painter,
        center,
        radius * 0.78,
        t::BRAND,
        0.020 + 0.008 * breath + 0.05 * pulse,
    );
    // Едва заметная дымка в кольце между диском и шкалой.
    // Начинается от центра и уходит под диск — так у неё нет резкой кромки
    t::radial_ring(
        painter,
        center,
        0.0,
        radius * 1.02,
        t::with_alpha(t::BRAND, 0.014 + 0.006 * breath),
        t::with_alpha(t::BRAND, 0.0),
    );

    // Базовое кольцо шкалы и насечки снаружи от него, как в макете
    painter.circle_stroke(center, radius, Stroke::new(1.0_f32, t::with_alpha(t::TEXT, 0.055)));
    let ticks = 64;
    for i in 0..ticks {
        let angle = i as f32 / ticks as f32 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        let major = i % 8 == 0;
        let dir = egui::vec2(angle.cos(), angle.sin());
        let inner = center + dir * (radius + 2.0);
        let outer = center + dir * (radius + radius * if major { 0.075 } else { 0.042 });
        let alpha = if major { 0.28 } else { 0.10 };
        painter.line_segment(
            [inner, outer],
            Stroke::new(1.0_f32, t::with_alpha(t::TEXT, alpha)),
        );
    }

    // Промежуточное кольцо — заполняет пустоту между шкалой и диском
    painter.circle_stroke(
        center,
        radius * 0.84,
        Stroke::new(1.0_f32, t::with_alpha(t::TEXT, 0.028)),
    );

    // Дуга уровня на шкале: в тишине короткий зелёный сегмент с точкой на конце
    let arc_radius = radius;
    let start = -std::f32::consts::FRAC_PI_2 - 0.06;
    let sweep = 0.66 + level * 1.6;
    let steps = 40;
    let mut points = Vec::with_capacity(steps + 1);
    for i in 0..=steps {
        let a = start + sweep * i as f32 / steps as f32;
        points.push(center + egui::vec2(a.cos(), a.sin()) * arc_radius);
    }
    let arc_color = if level > 0.92 { t::REC } else { t::LIVE };
    painter.add(egui::Shape::line(
        points.clone(),
        Stroke::new(3.0_f32, t::with_alpha(arc_color, 0.85)),
    ));
    if let Some(last) = points.last() {
        painter.circle_filled(*last, 2.6, arc_color);
    }

    // Сам диск
    let disc = radius * 0.68;
    // Ступенчатые круги дают мягкий верхний свет: диск читается как
    // подсвеченная сверху таблетка, а не как тёмная дыра
    let steps = 26;
    for i in 0..steps {
        let k = i as f32 / (steps - 1) as f32;
        let r = disc * (1.0 - 0.72 * k);
        let c = center - egui::vec2(0.0, disc * 0.24 * k);
        let g = k.powf(1.5);
        painter.circle_filled(
            c,
            r,
            egui::Color32::from_rgb(
                (16.0 + 44.0 * g) as u8,
                (16.0 + 44.0 * g) as u8,
                (21.0 + 52.0 * g) as u8,
            ),
        );
    }
    painter.circle_stroke(
        center,
        disc * 0.995,
        Stroke::new(1.0_f32, t::with_alpha(t::TEXT, 0.07)),
    );
    // Вертикальный шов-блик сверху — объём, как в макете
    painter.line_segment(
        [center, center - egui::vec2(0.0, disc * 0.93)],
        Stroke::new(1.0_f32, t::with_alpha(t::TEXT, 0.018)),
    );

    // Пульс Albus: фиолетовое кольцо с точкой внутри
    let ring = disc * 0.30;
    let dot = disc * 0.062 + disc * 0.02 * level;
    if speaking {
        for i in 0..3 {
            let phase = (time * 0.55 + i as f32 / 3.0) % 1.0;
            painter.circle_stroke(
                center,
                ring * (1.1 + phase * 1.5),
                Stroke::new(1.0_f32, t::with_alpha(t::LIVE, (1.0 - phase).powi(2) * 0.22)),
            );
        }
    }
    t::glow_circle(
        painter,
        center,
        ring * 0.62,
        t::BRAND,
        0.14 + 0.04 * breath + 0.08 * pulse,
    );
    let accent = if speaking { t::LIVE } else { t::BRAND };
    painter.circle_stroke(
        center,
        ring,
        Stroke::new(1.2_f32, t::with_alpha(accent, 0.65)),
    );
    // Скобка-дуга справа от кольца
    let mut bracket = Vec::with_capacity(9);
    for i in 0..=8 {
        let a = -0.75 + 1.5 * i as f32 / 8.0;
        bracket.push(center + egui::vec2(a.cos(), a.sin()) * (ring * 1.20));
    }
    painter.add(egui::Shape::line(
        bracket,
        Stroke::new(1.0_f32, t::with_alpha(accent, 0.45)),
    ));
    painter.circle_filled(center, dot, accent);

    painter.text(
        center + egui::vec2(0.0, disc * 0.52),
        egui::Align2::CENTER_CENTER,
        "P2P",
        egui::FontId::monospace(8.0),
        t::with_alpha(t::TEXT_MUTED, 0.6),
    );
}

pub fn draw_hero(
    ui: &mut egui::Ui,
    level: f32,
    speaking: bool,
    is_host: bool,
    _wave: &[f32],
    peer_count: usize,
    nick: &str,
) {
    let time = ui.input(|i| i.time) as f32;
    ui.vertical_centered(|ui| {
        let side = 250.0_f32.min(ui.available_width());
        let (rect, _) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
        draw_dial(ui.painter(), rect.center(), side * 0.40, level, speaking, time);

        ui.add_space(8.0);

        let title = if speaking {
            nick.to_string()
        } else if peer_count == 0 {
            "Эфир тих".to_string()
        } else {
            "Эфир открыт".to_string()
        };
        ui.label(egui::RichText::new(title).size(19.0).strong().color(t::TEXT));

        ui.add_space(3.0);
        ui.horizontal(|ui| {
            // Как в макете: строка «на линии» зелёная и в тишине
            let color = t::LIVE;
            let (dot_rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
            ui.painter().circle_filled(dot_rect.center(), 3.0, color);
            let status = if speaking {
                "Говорит сейчас".to_string()
            } else {
                format!("{} на линии", members_label(peer_count + 1))
            };
            ui.label(egui::RichText::new(status).size(10.5).color(color));
            if is_host {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("ХОСТ")
                        .size(8.5)
                        .strong()
                        .color(t::GOLD),
                );
            }
        });
    });
}

/// Полоса «линия» с подписью уровня — как в макете
pub fn draw_wave_strip(ui: &mut egui::Ui, wave: &[f32], speaking: bool, height: f32) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, Rounding::same(8.0), t::with_alpha(t::PAGE, 0.55));
    painter.text(
        rect.left_top() + egui::vec2(9.0, 7.0),
        egui::Align2::LEFT_TOP,
        "ЛИНИЯ",
        egui::FontId::proportional(8.5),
        t::TEXT_MUTED,
    );
    painter.text(
        rect.right_top() + egui::vec2(-9.0, 7.0),
        egui::Align2::RIGHT_TOP,
        "−∞ dB",
        egui::FontId::monospace(8.5),
        t::TEXT_MUTED,
    );
    let wave_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 9.0, rect.top() + 18.0),
        egui::pos2(rect.right() - 9.0, rect.bottom() - 8.0),
    );
    t::waveform(
        &painter,
        wave_rect,
        wave,
        if speaking { t::LIVE } else { t::with_alpha(t::LIVE, 0.75) },
    );
}
