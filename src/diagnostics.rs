//! Диагностика: счётчики, которые пишутся на ходу, и текстовый отчёт.
//!
//! Зачем отдельный модуль: числа должны быть видны пользователю, а не только в
//! консольном журнале. Когда «у него тихо» или «видео дёргается», разбор идёт
//! по этим строкам, а собеседник присылает свой отчёт кнопкой — поэтому отчёт
//! целиком копируется в буфер обмена одной кнопкой.
//!
//! Счётчики лежат в атомиках: их пишет аудиоколбэк (поток карты) и читает GUI,
//! замков между ними нет.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Счётчики приёма голоса: провалы буфера, маскированные кадры, перезапуски
/// потока собеседника и выброшенные пачкой пакеты.
#[derive(Default)]
pub struct RxStats {
    underruns: AtomicU64,
    concealed: AtomicU64,
    /// Из маскированных кадров — восстановленные in-band FEC (а не PLC)
    fec: AtomicU64,
    restarts: AtomicU64,
    drops: AtomicU64,
    /// Кадры, которых не было вовсе: дырка в `seq` больше, чем стоит
    /// восстанавливать FEC/PLC (или очередь переупорядочивания переполнилась).
    lost: AtomicU64,
    /// Кадры, дошедшие до декодера: вместе с `lost` даёт процент потерь.
    delivered: AtomicU64,
}

/// Значения счётчиков приёма на один момент.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RxSnapshot {
    pub underruns: u64,
    pub concealed: u64,
    /// Сколько из маскированных кадров восстановлено FEC
    pub fec: u64,
    pub restarts: u64,
    pub drops: u64,
    pub lost: u64,
    pub delivered: u64,
}

impl RxSnapshot {
    /// Сколько всего событий: ноль означает «приём чистый».
    pub fn events(&self) -> u64 {
        self.underruns + self.concealed + self.restarts + self.drops
    }

    /// Прирост между двумя снимками (`self` — раньше, `later` — позже).
    pub fn delta(&self, later: &RxSnapshot) -> RxSnapshot {
        RxSnapshot {
            underruns: later.underruns.saturating_sub(self.underruns),
            concealed: later.concealed.saturating_sub(self.concealed),
            fec: later.fec.saturating_sub(self.fec),
            restarts: later.restarts.saturating_sub(self.restarts),
            drops: later.drops.saturating_sub(self.drops),
            lost: later.lost.saturating_sub(self.lost),
            delivered: later.delivered.saturating_sub(self.delivered),
        }
    }
}

impl RxStats {
    pub fn underrun(&self) {
        self.underruns.fetch_add(1, Ordering::Relaxed);
    }

    pub fn concealed(&self) {
        self.concealed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn fec(&self) {
        self.fec.fetch_add(1, Ordering::Relaxed);
    }

    pub fn restart(&self) {
        self.restarts.fetch_add(1, Ordering::Relaxed);
    }

    pub fn drop(&self) {
        self.drops.fetch_add(1, Ordering::Relaxed);
    }

    /// Пропущено `n` кадров подряд: восстанавливать их нечем.
    pub fn lost(&self, n: u64) {
        self.lost.fetch_add(n, Ordering::Relaxed);
    }

    /// Кадр дошёл до декодера.
    pub fn delivered(&self) {
        self.delivered.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> RxSnapshot {
        RxSnapshot {
            underruns: self.underruns.load(Ordering::Relaxed),
            concealed: self.concealed.load(Ordering::Relaxed),
            fec: self.fec.load(Ordering::Relaxed),
            restarts: self.restarts.load(Ordering::Relaxed),
            drops: self.drops.load(Ordering::Relaxed),
            lost: self.lost.load(Ordering::Relaxed),
            delivered: self.delivered.load(Ordering::Relaxed),
        }
    }
}

/// Процент потерянных кадров голоса от всех ожидаемых.
///
/// `lost` — кадры, которых не было вовсе (дырка в `seq` больше, чем стоит
/// восстанавливать), `delivered` — дошедшие до декодера. Маскированные кадры
/// (FEC/PLC) не считаются потерянными: звук на их месте есть.
pub fn voice_loss_percent(delivered: u64, lost: u64) -> u32 {
    let total = delivered.saturating_add(lost);
    if total == 0 {
        return 0;
    }
    ((lost as u128 * 100 + total as u128 / 2) / total as u128) as u32
}

/// Одна строка про приём голоса — её показывают и панель, и отчёт, чтобы
/// числа на экране и в скопированном тексте не расходились.
pub fn rx_line(rx: &RxSnapshot) -> String {
    let total = rx.delivered.saturating_add(rx.lost);
    format!(
        "провалов {} · маскировано {} (FEC {}) · перезапусков {} · выброшено {} · потеряно {} из {} ({} %)",
        rx.underruns,
        rx.concealed,
        rx.fec,
        rx.restarts,
        rx.drops,
        rx.lost,
        total,
        voice_loss_percent(rx.delivered, rx.lost)
    )
}

/// Общая точка диагностики: создаётся в `main` и раздаётся тем, кто пишет
/// счётчики (аудиоколбэк) и тем, кто их показывает (панель GUI).
pub struct Diagnostics {
    pub rx: RxStats,
    started: std::time::Instant,
}

impl Diagnostics {
    pub fn new() -> Self {
        Self {
            rx: RxStats::default(),
            started: std::time::Instant::now(),
        }
    }

    /// Сколько приложение работает — в отчёт и в панель.
    pub fn uptime(&self) -> Duration {
        self.started.elapsed()
    }
}

impl Default for Diagnostics {
    fn default() -> Self {
        Self::new()
    }
}

/// Одно P2P-соединение глазами QUIC: путь, задержка, потери.
///
/// Это не «пинг приложения», а состояние самого транспорта: RTT считает QUIC,
/// `relay` показывает, идёт ли трафик через релей (медленнее и с лишним хопом),
/// `lost_packets` — потери на этом пути.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkLine {
    pub node_id: String,
    pub relay: bool,
    pub rtt_ms: u64,
    /// Сколько пакетов отправлено по этому пути: без него потери не в процентах.
    pub sent_packets: u64,
    pub lost_packets: u64,
    pub cwnd: u64,
    /// Оценка полосы контроллером перегрузки, бит/с.
    pub bandwidth_bps: Option<u64>,
}

/// Своя трансляция: что показывает превью, что успевает захват и сколько
/// уходит в сеть.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OwnVideo {
    pub show_fps: f32,
    pub target_fps: u32,
    pub encode_fps: u32,
    pub kbps: u32,
    pub decode_ms: f32,
    pub gpu_ms: f32,
    pub image_ms: f32,
}

/// Чужая трансляция на приёме.
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteVideo {
    pub peer: String,
    pub fps: f32,
    pub decode_ms: f32,
    pub gpu_ms: f32,
    pub image_ms: f32,
    pub buffer_ms: u32,
    pub frames: u64,
}

/// Всё, что попадает в отчёт. Поля простые: отчёт собирает GUI, а форматирует
/// [`render`] — её же проверяют тесты, без окна и без звука.
#[derive(Clone, Debug, Default)]
pub struct ReportInput {
    pub node_id: String,
    pub is_host: bool,
    pub members_in_room: usize,
    pub left_room: bool,
    pub friends_online: usize,
    pub friends_total: usize,
    pub links: Vec<LinkLine>,
    pub rx_total: RxSnapshot,
    /// Счётчики приёма за последнее окно (обычно 5 с).
    pub rx_window: RxSnapshot,
    pub rx_window_secs: f32,
    pub mic_device: Option<String>,
    pub input2_device: Option<String>,
    pub output_device: Option<String>,
    pub mic_muted: bool,
    pub mic_level: f32,
    pub opus_kbps: u32,
    pub own_video: Option<OwnVideo>,
    pub remote_video: Option<RemoteVideo>,
    pub errors: Vec<String>,
    pub uptime: Duration,
}

/// Потери в процентах от отправленного. Ноль отправленных — ноль потерь:
/// соединение только поднялось, считать ещё нечего.
pub fn loss_percent(sent: u64, lost: u64) -> u32 {
    if sent == 0 {
        return 0;
    }
    ((lost.min(sent) * 100) / sent) as u32
}

/// Короткий NodeID для строки отчёта: первые 8 символов, как в интерфейсе.
pub fn short_id(node_id: &str) -> String {
    if node_id.chars().count() > 8 {
        let head: String = node_id.chars().take(8).collect();
        format!("{head}…")
    } else {
        node_id.to_string()
    }
}

fn format_uptime(uptime: Duration) -> String {
    let secs = uptime.as_secs();
    format!("{:02}:{:02}:{:02}", secs / 3600, (secs / 60) % 60, secs % 60)
}

fn device_or_default(name: &Option<String>) -> String {
    match name {
        Some(name) => format!("«{name}»"),
        None => "по умолчанию".to_string(),
    }
}

/// Текст отчёта: то, что видит пользователь и что уходит собеседнику.
///
/// Порядок строк фиксирован: по нему сравнивают два отчёта, поэтому в тестах
/// проверяются ключевые строки, а не весь текст целиком.
pub fn render(input: &ReportInput) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Albus Audio · диагностика · сборка {}\n",
        env!("ALBUS_BUILD_STAMP")
    ));
    out.push_str(&format!(
        "Узел: {} (полный: {})\n",
        short_id(&input.node_id),
        input.node_id
    ));
    out.push_str(&format!(
        "Роль: {} · участников в комнате {} · «вышли из эфира»: {}\n",
        if input.is_host { "хост" } else { "гость" },
        input.members_in_room,
        if input.left_room { "да" } else { "нет" }
    ));
    out.push_str(&format!(
        "Друзья: {} из {} в сети\n",
        input.friends_online, input.friends_total
    ));

    if input.links.is_empty() {
        out.push_str("Соединения: нет\n");
    } else {
        out.push_str(&format!("Соединения: {}\n", input.links.len()));
        for link in &input.links {
            let path = if link.relay { "через релей" } else { "напрямую" };
            let bw = match link.bandwidth_bps {
                Some(bps) => format!(" · оценка {:.1} Мбит/с", bps as f64 / 1_000_000.0),
                None => String::new(),
            };
            let lost_pct = loss_percent(link.sent_packets, link.lost_packets);
            out.push_str(&format!(
                "  {} {path} · RTT {} мс · потерь {} ({}%) · окно {} КиБ{bw}\n",
                short_id(&link.node_id),
                link.rtt_ms,
                link.lost_packets,
                lost_pct,
                link.cwnd / 1024
            ));
        }
    }

    out.push_str(&format!(
        "Звук: микрофон {} · второй вход {} · динамики {}\n",
        device_or_default(&input.mic_device),
        device_or_default(&input.input2_device),
        device_or_default(&input.output_device)
    ));
    out.push_str(&format!(
        "Передача: {} кбит/с · уровень {:.2} · микрофон выключен: {}\n",
        input.opus_kbps,
        input.mic_level,
        if input.mic_muted { "да" } else { "нет" }
    ));
    out.push_str(&format!("Приём голоса (всего): {}\n", rx_line(&input.rx_total)));
    out.push_str(&format!(
        "Приём голоса (за {:.0} с): {}\n",
        input.rx_window_secs,
        rx_line(&input.rx_window)
    ));

    match &input.own_video {
        Some(v) => out.push_str(&format!(
            "Видео (своя): показ {:.0}/{:.0} FPS · захват {:.0} FPS · {:.1} Мбит/с · декод {:.1} мс · GPU {:.1} мс · картинка {:.1} мс\n",
            v.show_fps,
            v.target_fps as f32,
            v.encode_fps as f32,
            v.kbps as f32 / 1000.0,
            v.decode_ms,
            v.gpu_ms,
            v.image_ms
        )),
        None => out.push_str("Видео (своя): не идёт\n"),
    }
    match &input.remote_video {
        Some(v) => out.push_str(&format!(
            "Видео (приём): {} · {:.0} FPS · кадров {} · буфер {:.1} с · декод {:.1} мс · GPU {:.1} мс · картинка {:.1} мс\n",
            short_id(&v.peer),
            v.fps,
            v.frames,
            v.buffer_ms as f32 / 1000.0,
            v.decode_ms,
            v.gpu_ms,
            v.image_ms
        )),
        None => out.push_str("Видео (приём): нет\n"),
    }

    if input.errors.is_empty() {
        out.push_str("Ошибки: нет\n");
    } else {
        out.push_str(&format!("Ошибки: {}\n", input.errors.join(" · ")));
    }
    out.push_str(&format!("Время работы: {}\n", format_uptime(input.uptime)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn links() -> Vec<LinkLine> {
        vec![
            LinkLine {
                node_id: "cf8ab51aeb18a16c433bc425240293b9f67c77ce80545cac38ad676030fbc4ed".into(),
                relay: false,
                rtt_ms: 8,
                sent_packets: 100,
                lost_packets: 2,
                cwnd: 12_288,
                bandwidth_bps: Some(9_400_000),
            },
            LinkLine {
                node_id: "40e6933834977766dc7478e8f40d1f2bbc3d28fb28016d194de81db914659ed1".into(),
                relay: true,
                rtt_ms: 74,
                sent_packets: 0,
                lost_packets: 0,
                cwnd: 4_096,
                bandwidth_bps: None,
            },
        ]
    }

    #[test]
    fn rx_delta_counts_only_growth() {
        let before = RxSnapshot {
            underruns: 1,
            concealed: 2,
            fec: 1,
            restarts: 0,
            drops: 3,
            lost: 0,
            delivered: 0,
        };
        let later = RxSnapshot {
            underruns: 4,
            concealed: 2,
            fec: 3,
            restarts: 1,
            drops: 3,
            lost: 0,
            delivered: 0,
        };
        let d = before.delta(&later);
        assert_eq!(d.underruns, 3);
        assert_eq!(d.concealed, 0);
        assert_eq!(d.restarts, 1);
        assert_eq!(d.drops, 0);
    }

    #[test]
    fn rx_snapshot_sums_events() {
        let s = RxSnapshot {
            underruns: 1,
            concealed: 2,
            fec: 0,
            restarts: 3,
            drops: 4,
            lost: 0,
            delivered: 0,
        };
        assert_eq!(s.events(), 10);
        assert_eq!(RxSnapshot::default().events(), 0);
    }

    #[test]
    fn loss_percent_handles_no_traffic_and_full_loss() {
        assert_eq!(loss_percent(0, 0), 0, "ещё ничего не отправлено");
        assert_eq!(loss_percent(0, 5), 0, "без отправки процент не считается");
        assert_eq!(loss_percent(100, 2), 2);
        assert_eq!(loss_percent(1000, 12), 1, "округление вниз до целых");
        assert_eq!(loss_percent(50, 80), 100, "потерь больше, чем отправлено");
    }

    #[test]
    fn voice_loss_percent_rounds_to_nearest() {
        assert_eq!(voice_loss_percent(0, 0), 0, "приёма ещё не было");
        assert_eq!(voice_loss_percent(1000, 0), 0);
        assert_eq!(voice_loss_percent(999, 1), 0, "0.1 % — это ноль процентов");
        assert_eq!(voice_loss_percent(900, 100), 10);
        assert_eq!(voice_loss_percent(0, 5), 100, "всё потеряно");
        assert_eq!(voice_loss_percent(3, 1), 25);
    }

    #[test]
    fn rx_line_shows_lost_frames_and_percent() {
        let rx = RxSnapshot {
            underruns: 1,
            concealed: 2,
            fec: 1,
            restarts: 0,
            drops: 0,
            lost: 50,
            delivered: 950,
        };
        let line = rx_line(&rx);
        assert!(line.contains("потеряно 50 из 1000 (5 %)"), "строка: {line}");
        assert!(line.contains("маскировано 2 (FEC 1)"));
    }

    #[test]
    fn short_id_keeps_short_and_trims_long() {
        assert_eq!(short_id("abc"), "abc");
        assert_eq!(short_id("12345678"), "12345678");
        assert_eq!(short_id("123456789"), "12345678…");
    }

    #[test]
    fn report_has_every_section() {
        let input = ReportInput {
            node_id: "cf8ab51aeb18a16c433bc425240293b9f67c77ce80545cac38ad676030fbc4ed".into(),
            is_host: true,
            members_in_room: 2,
            left_room: false,
            friends_online: 1,
            friends_total: 3,
            links: links(),
            rx_total: RxSnapshot {
                underruns: 3,
                concealed: 12,
                fec: 7,
                restarts: 1,
                drops: 0,
                lost: 4,
                delivered: 996,
            },
            rx_window: RxSnapshot {
                concealed: 2,
                fec: 1,
                ..Default::default()
            },
            rx_window_secs: 5.0,
            mic_device: Some("1-2 (QUAD-CAPTURE)".into()),
            input2_device: None,
            output_device: Some("1-2 (QUAD-CAPTURE)".into()),
            mic_muted: false,
            mic_level: 0.12,
            opus_kbps: 64,
            own_video: Some(OwnVideo {
                show_fps: 58.0,
                target_fps: 60,
                encode_fps: 60,
                kbps: 8400,
                decode_ms: 3.1,
                gpu_ms: 4.2,
                image_ms: 0.9,
            }),
            remote_video: Some(RemoteVideo {
                peer: "40e6933834977766dc7478e8f40d1f2bbc3d28fb28016d194de81db914659ed1".into(),
                fps: 59.0,
                decode_ms: 2.9,
                gpu_ms: 3.4,
                image_ms: 0.7,
                buffer_ms: 2000,
                frames: 1234,
            }),
            errors: vec!["Сеть не запущена: timeout".into()],
            uptime: Duration::from_secs(3_725),
        };
        let text = render(&input);
        for needle in [
            "Albus Audio · диагностика",
            "Узел: cf8ab51a…",
            "Роль: хост",
            "участников в комнате 2",
            "Друзья: 1 из 3",
            "Соединения: 2",
            "напрямую · RTT 8 мс · потерь 2 (2%)",
            "через релей · RTT 74 мс",
            "оценка 9.4 Мбит/с",
            "Звук: микрофон «1-2 (QUAD-CAPTURE)»",
            "второй вход по умолчанию",
            "Передача: 64 кбит/с",
            "Приём голоса (всего): провалов 3 · маскировано 12 (FEC 7)",
            "Приём голоса (за 5 с): провалов 0 · маскировано 2",
            "Видео (своя): показ 58/60 FPS",
            "Видео (приём): 40e69338…",
            "Ошибки: Сеть не запущена: timeout",
            "Время работы: 01:02:05",
        ] {
            assert!(text.contains(needle), "в отчёте нет строки: {needle}");
        }
    }

    #[test]
    fn report_says_nothing_is_connected() {
        let text = render(&ReportInput::default());
        assert!(text.contains("Соединения: нет"));
        assert!(text.contains("Видео (своя): не идёт"));
        assert!(text.contains("Видео (приём): нет"));
        assert!(text.contains("Ошибки: нет"));
        assert!(text.contains("микрофон по умолчанию"));
    }

    #[test]
    fn report_marks_muted_microphone() {
        let input = ReportInput {
            mic_muted: true,
            ..Default::default()
        };
        assert!(render(&input).contains("микрофон выключен: да"));
    }
}





