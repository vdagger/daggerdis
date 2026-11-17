//! Визуальный язык приложения: палитра и примитивы отрисовки
//!
//! Токены повторяют утверждённый макет Albus: почти чёрная база,
//! панели на тон светлее, фиолетовый акцент и мятный «эфир».

use eframe::egui;
use egui::{Color32, Pos2, Rect, Rounding, Stroke};

pub const BG: Color32 = Color32::from_rgb(11, 11, 13);
pub const PAGE: Color32 = Color32::from_rgb(7, 7, 8);
pub const SURFACE: Color32 = Color32::from_rgb(21, 21, 25);
pub const SURFACE_HI: Color32 = Color32::from_rgb(30, 30, 37);
pub const INSET: Color32 = Color32::from_rgb(12, 13, 15);
/// Вложенная панель внутри карточки (чуть светлее SURFACE)
pub const INNER: Color32 = Color32::from_rgb(26, 26, 31);
/// Дорожка тонких слайдеров
pub const TRACK: Color32 = Color32::from_rgb(33, 33, 39);
/// Заполненная часть тонкого слайдера — макет серый, не брендовый
pub const TRACK_FILL: Color32 = Color32::from_rgb(118, 119, 124);
pub const STROKE: Color32 = Color32::from_rgb(38, 38, 44);
pub const STROKE_SOFT: Color32 = Color32::from_rgb(30, 30, 35);

pub const TEXT: Color32 = Color32::from_rgb(236, 237, 241);
pub const TEXT_DIM: Color32 = Color32::from_rgb(163, 165, 174);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(111, 113, 123);

pub const BRAND: Color32 = Color32::from_rgb(167, 139, 250);
pub const BRAND_2: Color32 = Color32::from_rgb(124, 92, 255);
pub const BRAND_DEEP: Color32 = Color32::from_rgb(109, 79, 232);
pub const LIVE: Color32 = Color32::from_rgb(124, 227, 176);
pub const REC: Color32 = Color32::from_rgb(255, 128, 128);
pub const GOLD: Color32 = Color32::from_rgb(229, 181, 103);

pub const RADIUS: f32 = 12.0;
pub const RADIUS_SM: f32 = 8.0;

pub fn with_alpha(color: Color32, alpha: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(
        color.r(),
        color.g(),
        color.b(),
        (alpha.clamp(0.0, 1.0) * 255.0) as u8,
    )
}

/// Мягкий фон карточки: заливка, тонкая рамка и поля
pub fn card_frame() -> egui::Frame {
    egui::Frame::none()
        .fill(SURFACE)
        .rounding(Rounding::same(RADIUS))
        .stroke(Stroke::new(1.0_f32, STROKE_SOFT))
        .inner_margin(egui::Margin::symmetric(12.0, 10.0))
}

/// Вложенное поле ввода на фоне карточки
pub fn inset_frame() -> egui::Frame {
    egui::Frame::none()
        .fill(INSET)
        .rounding(Rounding::same(RADIUS_SM))
        .stroke(Stroke::new(1.0_f32, STROKE_SOFT))
        .inner_margin(egui::Margin::symmetric(9.0, 6.0))
}

/// Компактная кнопка-плашка внутри карточки
pub fn chip_button(ui: &mut egui::Ui, text: &str, active: bool) -> egui::Response {
    let text_color = if active { BRAND } else { TEXT_DIM };
    let fill = if active {
        with_alpha(BRAND, 0.12)
    } else {
        TRACK
    };
    let btn = egui::Button::new(egui::RichText::new(text).size(10.5).strong().color(text_color))
        .fill(fill)
        .rounding(Rounding::same(7.0))
        .stroke(Stroke::new(
            1.0_f32,
            if active {
                with_alpha(BRAND, 0.28)
            } else {
                STROKE
            },
        ));
    ui.add(btn)
}

/// Главная кнопка действия
pub fn primary_button(ui: &mut egui::Ui, text: &str, size: egui::Vec2) -> egui::Response {
    ui.add_sized(
        size,
        egui::Button::new(
            egui::RichText::new(text)
                .size(12.0)
                .strong()
                .color(Color32::WHITE),
        )
        .fill(BRAND_2)
        .rounding(Rounding::same(10.0)),
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
        painter.add(egui::Shape::line(points, Stroke::new(3.0_f32, color)));
    }
}

/// Осциллограмма из истории уровней
pub fn waveform(painter: &egui::Painter, rect: Rect, samples: &[f32], color: Color32) {
    let peak = samples.iter().cloned().fold(0.0_f32, f32::max);
    if peak < 0.01 {
        painter.line_segment(
            [
                egui::pos2(rect.left(), rect.center().y),
                egui::pos2(rect.right(), rect.center().y),
            ],
            Stroke::new(1.2_f32, with_alpha(color, 0.55)),
        );
        return;
    }

    let n = samples.len().max(1);
    let step = rect.width() / n as f32;
    let mid = rect.center().y;
    let max_h = rect.height() * 0.45;
    let points: Vec<Pos2> = samples
        .iter()
        .enumerate()
        .map(|(i, sample)| {
            let amp = sample.clamp(0.0, 1.0).sqrt();
            let x = rect.left() + step * i as f32 + step * 0.5;
            let y = mid - (amp * max_h).max(0.5) * (if i % 2 == 0 { 1.0 } else { -1.0 });
            egui::pos2(x, y)
        })
        .collect();
    if points.len() > 1 {
        painter.add(egui::Shape::line(points, Stroke::new(1.6_f32, color)));
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

/// Три мини-столбика уровня — как в списке участников макета
pub fn level_bars(painter: &egui::Painter, rect: Rect, level: f32, color: Color32) {
    let level = level.clamp(0.0, 1.0);
    let w = 2.5;
    let gap = 3.0;
    let heights = [0.35_f32, 0.65, 1.0];
    for (i, k) in heights.iter().enumerate() {
        let h = 4.0 + (rect.height() - 4.0) * k;
        let x = rect.left() + i as f32 * (w + gap);
        let alpha = if level > *k * 0.75 { 0.95 } else { 0.22 };
        painter.rect_filled(
            Rect::from_min_max(
                egui::pos2(x, rect.bottom() - h),
                egui::pos2(x + w, rect.bottom()),
            ),
            Rounding::same(1.5),
            with_alpha(color, alpha),
        );
    }
}

/// Заголовок секции
pub fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(9.5)
            .strong()
            .color(TEXT_MUTED),
    );
}

/// Пунктирный разделитель между секциями карточки
pub fn divider(ui: &mut egui::Ui) {
    ui.add_space(2.0);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 1.0),
        egui::Sense::hover(),
    );
    ui.painter().line_segment(
        [rect.left_center(), rect.right_center()],
        Stroke::new(1.0_f32, STROKE_SOFT),
    );
    ui.add_space(2.0);
}

/// Небольшой бейдж-плашка
pub fn badge(ui: &mut egui::Ui, text: &str, color: Color32) {
    egui::Frame::none()
        .fill(with_alpha(color, 0.14))
        .rounding(Rounding::same(5.0))
        .stroke(Stroke::new(1.0_f32, with_alpha(color, 0.28)))
        .inner_margin(egui::Margin::symmetric(6.0, 2.0))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).size(9.0).strong().color(color));
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

    let knob_x = if *on {
        rect.right() - 11.0
    } else {
        rect.left() + 11.0
    };
    painter.circle_filled(
        egui::pos2(knob_x, rect.center().y),
        7.0,
        if *on { Color32::WHITE } else { TEXT_MUTED },
    );

    response
}

/// Иконки, нарисованные линиями: в шрифтах нет надёжных глифов
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Settings,
    Copy,
    Close,
    Chat,
    Phone,
    Send,
    Plus,
    More,
    Speaker,
    Sliders,
}

pub fn icon_button(ui: &mut egui::Ui, icon: Icon, size: f32, active: bool) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    let hovered = response.hovered();

    let painter = ui.painter_at(rect);
    if active || hovered {
        painter.rect_filled(
            rect,
            Rounding::same(7.0),
            if active {
                with_alpha(BRAND, 0.16)
            } else {
                with_alpha(Color32::WHITE, 0.05)
            },
        );
    }
    let color = if active {
        BRAND
    } else if hovered {
        TEXT
    } else {
        TEXT_DIM
    };
    draw_icon(&painter, rect.shrink(size * 0.28), icon, color);
    response
}

/// Рисует иконку в заданном квадрате
pub fn draw_icon(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let stroke = Stroke::new(1.4_f32, color);
    let c = rect.center();
    let r = rect.width() * 0.5;
    match icon {
        Icon::Settings => {
            for (dy, t) in [(-0.35_f32, 0.35_f32), (0.35, 0.65)] {
                let y = c.y + dy * r;
                painter.line_segment(
                    [egui::pos2(c.x - r, y), egui::pos2(c.x + r, y)],
                    stroke,
                );
                painter.circle_filled(egui::pos2(c.x - r + 2.0 * r * t, y), 2.2, color);
            }
        }
        Icon::Copy => {
            let back = Rect::from_min_size(
                egui::pos2(c.x - r * 0.7, c.y - r * 0.85),
                egui::vec2(r * 1.25, r * 1.25),
            );
            let front = Rect::from_min_size(
                egui::pos2(c.x - r * 0.25, c.y - r * 0.35),
                egui::vec2(r * 1.25, r * 1.25),
            );
            painter.rect_stroke(back, Rounding::same(2.0), Stroke::new(1.2_f32, with_alpha(color, 0.55)));
            painter.rect_stroke(front, Rounding::same(2.0), Stroke::new(1.4_f32, color));
        }
        Icon::Close => {
            painter.line_segment(
                [
                    egui::pos2(c.x - r * 0.7, c.y - r * 0.7),
                    egui::pos2(c.x + r * 0.7, c.y + r * 0.7),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(c.x + r * 0.7, c.y - r * 0.7),
                    egui::pos2(c.x - r * 0.7, c.y + r * 0.7),
                ],
                stroke,
            );
        }
        Icon::Chat => {
            let body = Rect::from_min_max(
                egui::pos2(rect.left(), rect.top() + r * 0.15),
                egui::pos2(rect.right(), rect.bottom() - r * 0.45),
            );
            painter.rect_stroke(body, Rounding::same(3.0), stroke);
            painter.line_segment(
                [
                    egui::pos2(body.left() + r * 0.35, body.bottom()),
                    egui::pos2(body.left() + r * 0.25, rect.bottom()),
                ],
                stroke,
            );
        }
        Icon::Phone => {
            // трубка: дуга снизу и два «уха» по краям
            let radius = r * 0.85;
            let mut points = Vec::new();
            for i in 0..=12 {
                let a = std::f32::consts::PI * (1.08 + 0.84 * i as f32 / 12.0);
                points.push(egui::pos2(
                    c.x + radius * a.cos(),
                    c.y + radius * a.sin() + r * 0.1,
                ));
            }
            painter.add(egui::Shape::line(points, Stroke::new(1.6_f32, color)));
            for (x, y, dx, dy) in [
                (-0.86_f32, 0.05_f32, -0.25_f32, -0.45_f32),
                (0.86, 0.05, 0.25, -0.45),
            ] {
                painter.line_segment(
                    [
                        egui::pos2(c.x + x * r, c.y + y * r),
                        egui::pos2(c.x + (x + dx) * r, c.y + (y + dy) * r),
                    ],
                    Stroke::new(1.6_f32, color),
                );
            }
        }
        Icon::Plus => {
            painter.line_segment(
                [egui::pos2(c.x - r * 0.7, c.y), egui::pos2(c.x + r * 0.7, c.y)],
                stroke,
            );
            painter.line_segment(
                [egui::pos2(c.x, c.y - r * 0.7), egui::pos2(c.x, c.y + r * 0.7)],
                stroke,
            );
        }
        Icon::More => {
            for dx in [-0.6_f32, 0.0, 0.6] {
                painter.circle_filled(egui::pos2(c.x + r * dx, c.y), 2.0, color);
            }
        }
        Icon::Speaker => {
            // корпус с раструбом
            let body = vec![
                egui::pos2(c.x - r * 0.75, c.y - r * 0.28),
                egui::pos2(c.x - r * 0.35, c.y - r * 0.28),
                egui::pos2(c.x + r * 0.05, c.y - r * 0.72),
                egui::pos2(c.x + r * 0.05, c.y + r * 0.72),
                egui::pos2(c.x - r * 0.35, c.y + r * 0.28),
                egui::pos2(c.x - r * 0.75, c.y + r * 0.28),
            ];
            painter.add(egui::Shape::convex_polygon(body, color, Stroke::NONE));
            // две дуги звука
            for (k, alpha) in [(0.45_f32, 0.9_f32), (0.78, 0.55)] {
                let mut points = Vec::new();
                for i in 0..=8 {
                    let a = -0.9 + 1.8 * i as f32 / 8.0;
                    points.push(egui::pos2(
                        c.x + r * 0.1 + r * k * a.cos() * 0.8,
                        c.y + r * k * a.sin(),
                    ));
                }
                painter.add(egui::Shape::line(
                    points,
                    Stroke::new(1.4_f32, with_alpha(color, alpha)),
                ));
            }
        }
        Icon::Sliders => {
            // Две линии с ползунком — как значок микшера в макете
            for (k, knob_x) in [(0.0_f32, 0.30_f32), (1.0, 0.66)] {
                let y = c.y + (k - 0.5) * r * 0.75;
                painter.line_segment(
                    [
                        egui::pos2(c.x - r * 0.75, y),
                        egui::pos2(c.x + r * 0.75, y),
                    ],
                    Stroke::new(1.3_f32, with_alpha(color, 0.65)),
                );
                painter.circle_filled(
                    egui::pos2(c.x + r * (knob_x - 0.5) * 1.5, y),
                    r * 0.16,
                    color,
                );
            }
        }
        Icon::Send => {
            let shape = vec![
                egui::pos2(rect.left() + r * 0.1, rect.top() + r * 0.1),
                egui::pos2(rect.right(), c.y),
                egui::pos2(rect.left() + r * 0.1, rect.bottom() - r * 0.1),
                egui::pos2(rect.left() + r * 0.38, c.y),
            ];
            painter.add(egui::Shape::convex_polygon(shape, color, Stroke::NONE));
        }
    }
}

/// Квадратная плашка-аватар с первой буквой имени, как в макете:
/// сплошная заливка цветом по хешу имени
pub fn avatar(ui: &mut egui::Ui, name: &str, size: f32, speaking: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let initial: String = name
        .trim()
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".to_string());
    let hue = name.bytes().fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    let palette = [
        BRAND_2,
        Color32::from_rgb(58, 178, 126),
        Color32::from_rgb(196, 140, 62),
        Color32::from_rgb(96, 96, 112),
    ];
    let color = palette[(hue as usize) % palette.len()];
    let rounding = Rounding::same(size * 0.34);

    painter.rect_filled(rect, rounding, color);
    if speaking {
        painter.rect_stroke(
            rect,
            rounding,
            Stroke::new(1.4_f32, with_alpha(Color32::WHITE, 0.7)),
        );
    }
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        initial,
        egui::FontId::proportional(size * 0.42),
        Color32::WHITE,
    );
}

/// Подпись под органом управления: маленький серый текст
pub fn micro(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).size(9.5).color(TEXT_MUTED));
}
