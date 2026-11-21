//! Показ видео: декодирование H.264 и текстура egui.
//!
//! `VideoSession` — своя трансляция (захват + превью), `RemoteView` — чужой
//! поток, пришедший по сети. Оба используют общий `Renderer`.

use std::time::{Duration, Instant};

use eframe::egui;

use crate::video::publisher::{ScreenPublisher, StreamInfo};
use crate::video::win_d3d::{
    create_device, nv12_texture, upload_nv12, Converter, D3dDevice, FORMAT_BGRA, FORMAT_NV12,
};
use crate::video::win_decode::MfDecoder;
use crate::video::EncodedUnit;

/// Декод кадров и обновление текстуры egui.
pub struct Renderer {
    decoder: MfDecoder,
    d3d: D3dDevice,
    nv12_tex: windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
    converter: Converter,
    texture: Option<egui::TextureHandle>,
    width: u32,
    height: u32,
    /// Размер кадра после GPU-конверсии (превью — вдвое меньше потока).
    out_width: u32,
    out_height: u32,
    fps_window_start: Instant,
    fps_window_frames: u32,
    fps: f32,
    /// Стоимость последнего кадра по этапам, мс (диагностика).
    decode_ms: f32,
    readback_ms: f32,
    image_ms: f32,
    /// Частота апдейтов GUI, в которых этот показ участвовал: по ней видно,
    /// упёрся ли показ в свой период или в редкие кадры окна.
    update_window_start: Instant,
    update_window_ticks: u32,
    update_fps: f32,
    /// Конверсия на GPU запущена, кадр ещё не забран в память.
    pending_since: Option<Instant>,
    /// Декодированные кадры, ждущие отправки в D3D. Очередь, а не один слот:
    /// сеть отдаёт кадры пачками, два могут стать готовыми в одном апдейте,
    /// и второй молча пропадал — показ честно терял ~16 кадров из 60.
    queued: std::collections::VecDeque<Vec<u8>>,
    /// Сколько раз за окно GPU не был готов сразу (показ ждал) и сколько
    /// кадров выбросили из очереди показа. Это и есть ответ на «почему не 60».
    readback_waits: u32,
    dropped: u32,
    /// Дамп уже сделан (диагностика ALBUS_VIDEO_DUMP).
    /// Дампы уже сделаны (диагностика ALBUS_VIDEO_DUMP).
    dump_nv12: bool,
    dump_bgra: bool,
    pub last_error: Option<String>,
}

impl Renderer {
    /// `out_scale` — во сколько раз уменьшить кадр после конверсии (1 = как в
    /// потоке). Превью показывает картинку в маленьком окне, и readback +
    /// конвертация в `ColorImage` + загрузка текстуры на полном 1080p стоят
    /// дороже, чем даёт видимая разница.
    pub fn new(info: &StreamInfo, out_scale: u32) -> Result<Self, String> {
        let scale = out_scale.max(1);
        let out_w = (info.width / scale) & !1;
        let out_h = (info.height / scale) & !1;
        let decoder = MfDecoder::new(info)?;
        let d3d = create_device(None)?;
        let nv12_tex = nv12_texture(&d3d.device, info.width, info.height)?;
        let converter = Converter::new(
            &d3d.device,
            &d3d.context,
            FORMAT_NV12,
            info.width,
            info.height,
            FORMAT_BGRA,
            out_w,
            out_h,
            info.fps,
        )?;
        Ok(Self {
            decoder,
            d3d,
            nv12_tex,
            converter,
            texture: None,
            width: info.width,
            height: info.height,
            out_width: out_w,
            out_height: out_h,
            fps_window_start: Instant::now(),
            fps_window_frames: 0,
            fps: 0.0,
            decode_ms: 0.0,
            readback_ms: 0.0,
            image_ms: 0.0,
            update_window_start: Instant::now(),
            update_window_ticks: 0,
            update_fps: 0.0,
            pending_since: None,
            queued: std::collections::VecDeque::new(),
            readback_waits: 0,
            dropped: 0,
            dump_nv12: false,
            dump_bgra: false,
            last_error: None,
        })
    }

    pub fn feed(&mut self, units: &[EncodedUnit], ctx: &egui::Context) {
        let t_decode = Instant::now();
        let mut latest: Option<Vec<u8>> = None;
        for unit in units {
            match self.decoder.push(unit) {
                Ok(frames) => {
                    if let Some(frame) = frames.into_iter().last() {
                        latest = Some(frame);
                    }
                }
                Err(e) => {
                    self.last_error = Some(e);
                    break;
                }
            }
        }
        // Диагностика: один дамп декодированного кадра и картинки в BGRA,
        // ALBUS_VIDEO_DUMP=<префикс>. Нужно для «стрим показывает полосы».
        if !self.dump_nv12 {
            if let Ok(prefix) = std::env::var("ALBUS_VIDEO_DUMP") {
                if let Some(nv12) = latest.as_ref() {
                    let _ = std::fs::write(format!("{prefix}.feed.nv12"), nv12);
                    println!(
                        "[ВИДЕО-DBG] NV12 {} байт, ожидалось {} ({}x{})",
                        nv12.len(),
                        self.width as usize * self.height as usize * 3 / 2,
                        self.width,
                        self.height
                    );
                    self.dump_nv12 = true;
                }
            }
        }

        if !units.is_empty() {
            self.decode_ms = t_decode.elapsed().as_secs_f32() * 1000.0;
        }

        // Короткий кадр (ошибка декодера) нельзя отдавать в D3D: UpdateSubresource
        // прочитает за концом буфера. Лишние байты — тоже признак беды (например,
        // декодер отдал BGRA, а его читают как NV12): получились бы диагональные
        // полосы. Лучше пропустить кадр и сказать об этом.
        if let Some(nv12) = latest {
            let need = self.width as usize * self.height as usize * 3 / 2;
            if nv12.len() < need {
                self.last_error = Some(format!(
                    "короткий NV12-кадр: {} из {} байт",
                    nv12.len(),
                    need
                ));
            } else if nv12.len() > need + 64 {
                self.last_error = Some(format!(
                    "кадр не того формата: {} байт вместо {} ({}x{})",
                    nv12.len(),
                    need,
                    self.width,
                    self.height
                ));
            } else {
                // Показ берёт не больше одного кадра за апдейт, поэтому
                // очередь короткая: два-три кадра — это 30-50 мс задержки,
                // зато пачка из сети не теряет кадр целиком.
                if self.queued.len() >= 3 {
                    self.queued.pop_front();
                    self.dropped += 1;
                }
                self.queued.push_back(nv12);
            }
        }

        self.pump(ctx);
    }

    /// Забирает готовый кадр, не дожидаясь GPU (блокирующий `Map` стоит
    /// десятки миллисекунд на кадр — из-за него превью шло ~30 fps).
    pub fn tick(&mut self, ctx: &egui::Context) {
        self.update_window_ticks += 1;
        let dt = self.update_window_start.elapsed();
        if dt >= Duration::from_secs(1) {
            self.update_fps = self.update_window_ticks as f32 / dt.as_secs_f32();
            self.update_window_start = Instant::now();
            self.update_window_ticks = 0;
        }
        self.pump(ctx);
    }

    /// Отправляет на GPU самый свежий отложенный кадр, если тот готов и показ
    /// не упёрся в частоту потока. Вызывается каждый апдейт: даже без новых
    /// кадров из сети отложенный кадр должен дойти до экрана.
    fn pump(&mut self, ctx: &egui::Context) {
        // кадр от прошлой конверсии — если GPU уже досчитал
        self.poll_readback(ctx);

        // предыдущая конверсия ещё не забрана — второй раз на GPU не отправляем
        if self.pending_since.is_some() {
            return;
        }
        // Потолок показа задаёт сам поток: каждый кадр показывается один раз,
        // и приходит их не больше fps. Искусственный период 0.75·(1/fps) здесь
        // только вредил: при 74 Гц апдейты идут через 13.5 мс, любой апдейт на
        // 12.5 мс отбрасывался и кадр ждал ещё один — показ держался на 44-46
        // FPS при потоке 60.
        let Some(nv12) = self.queued.pop_front() else {
            return;
        };
        upload_nv12(&self.d3d.context, &self.nv12_tex, self.width, &nv12);
        if let Err(e) = self.converter.convert(&self.nv12_tex) {
            self.last_error = Some(e);
            return;
        }
        if let Err(e) = self.converter.copy_to_staging() {
            self.last_error = Some(e);
            return;
        }
        self.pending_since = Some(Instant::now());
    }

    fn poll_readback(&mut self, ctx: &egui::Context) {
        let Some(since) = self.pending_since else {
            return;
        };
        // страховка: если GPU так и не отдал кадр, не залипаем навсегда
        if since.elapsed() > Duration::from_millis(500) {
            self.pending_since = None;
            return;
        }
        let t_read = Instant::now();
        let bgra = match self.converter.read_back_nowait() {
            Ok(Some(b)) => b,
            Ok(None) => {
                // GPU ещё считает: кадр не потерян, покажем следующим апдейтом.
                self.readback_waits += 1;
                return;
            }
            Err(e) => {
                self.pending_since = None;
                self.last_error = Some(e);
                return;
            }
        };
        self.readback_ms = t_read.elapsed().as_secs_f32() * 1000.0;
        self.pending_since = None;

        let t_image = Instant::now();
        if let Ok(prefix) = std::env::var("ALBUS_VIDEO_DUMP") {
            if !self.dump_bgra {
                let _ = std::fs::write(format!("{prefix}.show.bgra"), &bgra);
                println!(
                    "[ВИДЕО-DBG] BGRA {} байт, ожидалось {} ({}x{})",
                    bgra.len(),
                    self.out_width as usize * self.out_height as usize * 4,
                    self.out_width,
                    self.out_height
                );
                self.dump_bgra = true;
            }
        }
        let image = bgra_to_image(&bgra, self.out_width, self.out_height);
        match &mut self.texture {
            Some(t) => t.set(image, egui::TextureOptions::LINEAR),
            None => {
                self.texture = Some(ctx.load_texture(
                    "video-frame",
                    image,
                    egui::TextureOptions::LINEAR,
                ));
            }
        }
        self.image_ms = t_image.elapsed().as_secs_f32() * 1000.0;

        // настоящий fps: кадры за последнюю секунду, а не суммарный счётчик
        self.fps_window_frames += 1;
        let dt = self.fps_window_start.elapsed();
        if dt >= Duration::from_secs(1) {
            self.fps = self.fps_window_frames as f32 / dt.as_secs_f32();
            self.fps_window_start = Instant::now();
            self.fps_window_frames = 0;
        }
    }

    pub fn texture(&self) -> Option<&egui::TextureHandle> {
        self.texture.as_ref()
    }

    pub fn size(&self) -> (u32, u32) {
        (self.out_width, self.out_height)
    }

    /// Кадров в секунду за последнее окно измерения.
    pub fn fps(&self) -> f32 {
        self.fps
    }

    /// Апдейтов GUI в секунду. Если показ ниже потока, а это число высокое —
    /// виноват показ; если низкое — само окно перерисовывается редко.
    pub fn update_fps(&self) -> f32 {
        self.update_fps
    }

    /// Стоимость этапов последнего кадра: декод, чтение с GPU, картинка+текстура.
    pub fn timings(&self) -> (f32, f32, f32) {
        (self.decode_ms, self.readback_ms, self.image_ms)
    }

    /// Счётчики с прошлого вызова за окно журнала: сколько раз показ ждал
    /// GPU и сколько кадров не поместилось в очередь показа. Без них «44 из
    /// 60» — догадка, с ними — причина.
    pub fn take_diag(&mut self) -> (u32, u32) {
        let out = (self.readback_waits, self.dropped);
        self.readback_waits = 0;
        self.dropped = 0;
        out
    }
}

/// Своя трансляция: захват экрана, превью и кадры для сети.
pub struct VideoSession {
    publisher: ScreenPublisher,
    renderer: Renderer,
    pub source: String,
}

impl VideoSession {
    /// Целевой битрейт для адаптации канала: сюда пишет `VideoLink`.
    pub fn bitrate_target(&self) -> std::sync::Arc<std::sync::atomic::AtomicU32> {
        self.publisher.bitrate.clone()
    }

    /// Битрейт пресета — потолок адаптации.
    pub fn bitrate_base(&self) -> u32 {
        self.publisher.bitrate_base
    }

    /// Флаг «выдай IDR»: сюда пишет `VideoLink`, когда подписался зритель.
    pub fn keyframe_target(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        self.publisher.keyframe.clone()
    }

    /// Пресет, с которым идёт поток (для карточки и журнала).
    pub fn preset(&self) -> crate::video::VideoPreset {
        self.publisher.preset
    }

    /// `window` — транслировать окно вместо всего экрана.
    pub fn start(
        fps: u32,
        window: Option<isize>,
        preset: Option<crate::video::VideoPreset>,
        keyframe: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<Self, String> {
        let publisher = ScreenPublisher::start(fps, window, preset, keyframe)?;
        // Превью своей трансляции: половинное разрешение нужно только большим
        // потокам (1080p и выше), чтобы чтение с GPU и загрузка текстуры не
        // съедали 60 кадров. Для окна 1110x610 половинное превью — это 555 px,
        // которые потом растягиваются в окно показа, и текст выглядит
        // «пикселями»: там дешевле показать кадр как есть.
        let scale = if publisher.info.width as u64 * publisher.info.height as u64
            >= 1280 * 720
        {
            2
        } else {
            1
        };
        let renderer = Renderer::new(&publisher.info, scale)?;
        Ok(Self {
            source: publisher.source.clone(),
            publisher,
            renderer,
        })
    }

    pub fn info(&self) -> &StreamInfo {
        &self.publisher.info
    }

    /// Забирает закодированные кадры, показывает последний.
    /// Возвращённые кадры уходят в сеть.
    pub fn poll(&mut self, ctx: &egui::Context) -> Vec<EncodedUnit> {
        self.renderer.tick(ctx);
        let mut units = Vec::new();
        let mut budget = 4;
        while budget > 0 {
            match self.publisher.rx.try_recv() {
                Ok(u) => {
                    units.push(u);
                    budget -= 1;
                }
                Err(_) => break,
            }
        }
        if !units.is_empty() {
            self.renderer.feed(&units, ctx);
        }
        units
    }

    pub fn texture(&self) -> Option<&egui::TextureHandle> {
        self.renderer.texture()
    }

    pub fn size(&self) -> (u32, u32) {
        self.renderer.size()
    }

    pub fn fps(&self) -> f32 {
        self.renderer.fps()
    }

    pub fn update_fps(&self) -> f32 {
        self.renderer.update_fps()
    }

    pub fn timings(&self) -> (f32, f32, f32) {
        self.renderer.timings()
    }

    /// (ждали GPU, выброшено кадров) с прошлого вызова — для журнала.
    pub fn take_diag(&mut self) -> (u32, u32) {
        self.renderer.take_diag()
    }

    /// Фактическая частота кодирования (не показа).
    pub fn encode_fps(&self) -> u32 {
        self.publisher
            .encode_fps
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Сколько килобит в секунду уходит в сеть (оценка энкодера за секунду).
    pub fn encode_kbps(&self) -> u32 {
        self.publisher
            .encode_kbps
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn last_error(&self) -> Option<&String> {
        self.renderer.last_error.as_ref()
    }
}

/// Чужая трансляция, пришедшая по сети.
pub struct RemoteView {
    renderer: Renderer,
    pub peer: String,
}

impl RemoteView {
    pub fn new(info: &StreamInfo, peer: String) -> Result<Self, String> {
        Ok(Self {
            // чужой поток — в полном разрешении: его смотрят в отдельном окне
            renderer: Renderer::new(info, 1)?,
            peer,
        })
    }

    pub fn feed(&mut self, unit: EncodedUnit, ctx: &egui::Context) {
        self.renderer.feed(std::slice::from_ref(&unit), ctx);
    }

    /// Забрать готовый кадр, не дожидаясь GPU.
    pub fn tick(&mut self, ctx: &egui::Context) {
        self.renderer.tick(ctx);
    }

    pub fn texture(&self) -> Option<&egui::TextureHandle> {
        self.renderer.texture()
    }

    pub fn size(&self) -> (u32, u32) {
        self.renderer.size()
    }

    pub fn fps(&self) -> f32 {
        self.renderer.fps()
    }

    pub fn update_fps(&self) -> f32 {
        self.renderer.update_fps()
    }

    /// Стоимость этапов показа (декод, чтение с GPU, картинка), мс.
    /// Нужна, чтобы по логу/экрану зрителя отличить слабый компьютер
    /// (дорогой декод или чтение с GPU) от потерь в сети.
    pub fn timings(&self) -> (f32, f32, f32) {
        self.renderer.timings()
    }

    /// (ждали GPU, выброшено кадров) с прошлого вызова — для журнала.
    pub fn take_diag(&mut self) -> (u32, u32) {
        self.renderer.take_diag()
    }

    pub fn last_error(&self) -> Option<&String> {
        self.renderer.last_error.as_ref()
    }
}

/// BGRA с GPU → ColorImage. egui ждёт RGBA, поэтому меняем B и R местами.
fn bgra_to_image(bgra: &[u8], width: u32, height: u32) -> egui::ColorImage {
    let pixels = bgra
        .chunks_exact(4)
        .map(|p| egui::Color32::from_rgb(p[2], p[1], p[0]))
        .collect();
    egui::ColorImage {
        size: [width as usize, height as usize],
        pixels,
    }
}
