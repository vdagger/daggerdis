//! Визуальный язык приложения: палитра и примитивы отрисовки

use eframe::egui;
use egui::{Color32, Pos2, Rect, Rounding, Stroke};

pub const BG: Color32 = Color32::from_rgb(9, 10, 13);
pub const SURFACE: Color32 = Color32::from_rgb(20, 22, 27);
pub const SURFACE_HI: Color32 = Color32::from_rgb(28, 31, 37);
pub const STROKE: Color32 = Color32::from_rgb(40, 44, 52);
pub const TEXT: Color32 = Color32::from_rgb(240, 242, 246);
pub const TEXT_DIM: Color32 = Color32::from_rgb(150, 157, 170);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(98, 105, 118);

pub const BRAND: Color32 = Color32::from_rgb(198, 74, 255);
pub const BRAND_DEEP: Color32 = Color32::from_rgb(124, 58, 237);
pub const LIVE: Color32 = Color32::from_rgb(52, 211, 153);
pub const REC: Color32 = Color32::from_rgb(239, 68, 68);
pub const GOLD: Color32 = Color32::from_rgb(234, 179, 8);

pub fn with_alpha(color: Color32, alpha: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(
        color.r(),
        color.g(),
        color.b(),
        (alpha.clamp(0.0, 1.0) * 255.0) as u8,
    )
}

/// Сплошной радиальный градиент: цвет интерполируется по вершинам mesh,
/// поэтому переход плавный и без концентрических ступенек
pub fn radial_fill(
    painter: &egui::Painter,
    center: Pos2,
    radius: f32,
    center_color: Color32,
    rim_color: Color32,
) {
    const SEGMENTS: usize = 64;
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(center, center_color);
    for i in 0..=SEGMENTS {
        let angle = i as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
        mesh.colored_vertex(
            center + egui::vec2(angle.cos(), angle.sin()) * radius,
            rim_color,
        );
    }
    for i in 0..SEGMENTS {
        mesh.add_triangle(0, 1 + i as u32, 2 + i as u32);
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// Кольцевое свечение: от внутреннего радиуса к внешнему
pub fn radial_ring(
    painter: &egui::Painter,
    center: Pos2,
    inner_radius: f32,
    outer_radius: f32,
    inner_color: Color32,
    outer_color: Color32,
) {
    const SEGMENTS: usize = 72;
    let mut mesh = egui::Mesh::default();
    for i in 0..=SEGMENTS {
        let angle = i as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
        let dir = egui::vec2(angle.cos(), angle.sin());
        mesh.colored_vertex(center + dir * inner_radius, inner_color);
        mesh.colored_vertex(center + dir * outer_radius, outer_color);
    }
    for i in 0..SEGMENTS {
        let base = (i * 2) as u32;
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base + 2, base + 1, base + 3);
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// Мягкое круглое свечение
pub fn glow_circle(
    painter: &egui::Painter,
    center: Pos2,
    radius: f32,
    color: Color32,
    strength: f32,
) {
    radial_ring(
        painter,
        center,
        radius * 0.2,
        radius,
        with_alpha(color, strength),
        with_alpha(color, 0.0),
    );
}

/// Медленный «радарный» след, вращающийся вокруг аватара
pub fn sweep_arc(
    painter: &egui::Painter,
    center: Pos2,
    radius: f32,
    time: f32,
    color: Color32,
    strength: f32,
) {
    const TICKS: usize = 28;
    let head = time * 0.55;
    for i in 0..TICKS {
        let f = i as f32 / TICKS as f32;
        let angle = head - (1.0 - f) * 1.6;
        let alpha = strength * f * f * f;
        let dir = egui::vec2(angle.cos(), angle.sin());
        painter.line_segment(
            [center + dir * (radius - 4.0), center + dir * (radius + 4.0)],
            Stroke::new(1.5_f32, with_alpha(color, alpha)),
        );
    }
}

/// Свечение по контуру — для активных состояний
pub fn glow_rect(
    painter: &egui::Painter,
    rect: Rect,
    rounding: Rounding,
    color: Color32,
    strength: f32,
) {
    for (grow, k, width) in [
        (0.0_f32, 0.55_f32, 2.0_f32),
        (3.0, 0.3, 1.5),
        (6.0, 0.15, 1.0),
    ] {
        painter.rect_stroke(
            rect.expand(grow),
            Rounding::same(rounding.nw + grow),
            Stroke::new(width, with_alpha(color, strength * k)),
        );
    }
}

/// Дуга-индикатор уровня вокруг аватара
pub fn level_arc(
    painter: &egui::Painter,
    center: Pos2,
    radius: f32,
    level: f32,
    color: Color32,
) {
    const START: f32 = std::f32::consts::PI * 0.75;
    const SWEEP: f32 = std::f32::consts::PI * 1.5;

    let steps = 64;
    let mut points = Vec::with_capacity(steps + 1);
    let filled = level.clamp(0.0, 1.0) as f32;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let angle = START + SWEEP * t * filled;
        points.push(center + egui::vec2(angle.cos(), angle.sin()) * radius);
    }

    if points.len() > 1 {
        painter.add(egui::Shape::line(
            points,
            Stroke::new(2.5_f32, color),
        ));
    }
}

/// Осциллограмма из истории уровней
pub fn waveform(painter: &egui::Painter, rect: Rect, samples: &[f32], color: Color32) {
    let peak = samples.iter().cloned().fold(0.0_f32, f32::max);
    if peak < 0.01 {
        painter.line_segment(
            [
                egui::pos2(rect.left() + rect.width() * 0.15, rect.center().y),
                egui::pos2(rect.right() - rect.width() * 0.15, rect.center().y),
            ],
            Stroke::new(1.0_f32, with_alpha(color, 0.22)),
        );
        return;
    }

    let n = samples.len().max(1);
    let step = rect.width() / n as f32;
    let bar_w = (step * 0.5).max(1.5);
    let mid = rect.center().y;
    let max_h = rect.height() * 0.45;

    for (i, sample) in samples.iter().enumerate() {
        let amp = sample.clamp(0.0, 1.0).sqrt();
        let h = (amp * max_h).max(1.0);
        let x = rect.left() + step * i as f32 + step * 0.5;
        painter.rect_filled(
            Rect::from_center_size(egui::pos2(x, mid), egui::vec2(bar_w, h * 2.0)),
            Rounding::same(bar_w * 0.5),
            with_alpha(color, 0.35 + 0.65 * amp),
        );
    }
}

/// Полоса уровня с зонами: зелёная — рабочая, жёлтая — близко к клипу, красная — клип
pub fn vu_meter(painter: &egui::Painter, rect: Rect, level: f32) {
    painter.rect_filled(
        rect,
        Rounding::same(2.0),
        Color32::from_rgba_unmultiplied(255, 255, 255, 16),
    );

    let level = level.clamp(0.0, 1.0) as f32;
    for (from, to, color) in [(0.0, 0.7, LIVE), (0.7, 0.9, GOLD), (0.9, 1.0, REC)] {
        let x0 = rect.left() + rect.width() * from;
        let x1 = rect.left() + rect.width() * level.clamp(from, to);
        if x1 > x0 + 0.5 {
            painter.rect_filled(
                Rect::from_min_max(egui::pos2(x0, rect.top()), egui::pos2(x1, rect.bottom())),
                Rounding::same(2.0),
                color,
            );
        }
    }
}

/// Заголовок секции
pub fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(10.0)
            .strong()
            .color(TEXT_MUTED),
    );
}

/// Небольшой бейдж-плашка
pub fn badge(ui: &mut egui::Ui, text: &str, color: Color32) {
    egui::Frame::none()
        .fill(with_alpha(color, 0.14))
        .rounding(Rounding::same(5.0))
        .inner_margin(egui::Margin::symmetric(6.0, 2.0))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).size(9.5).strong().color(color));
        });
}

/// Переключатель вместо чекбокса
pub fn switch(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let size = egui::vec2(38.0, 20.0);
    let (rect, mut response) = ui.allocate_exact_size(size, egui::Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }

    let painter = ui.painter_at(rect);
    let track = if *on { with_alpha(LIVE, 0.85) } else { SURFACE_HI };
    painter.rect_filled(rect, Rounding::same(10.0), track);
    painter.rect_stroke(rect, Rounding::same(10.0), Stroke::new(1.0_f32, STROKE));

    let knob_x = if *on { rect.right() - 11.0 } else { rect.left() + 11.0 };
    painter.circle_filled(
        egui::pos2(knob_x, rect.center().y),
        7.0,
        if *on { Color32::WHITE } else { TEXT_MUTED },
    );

    response
}
