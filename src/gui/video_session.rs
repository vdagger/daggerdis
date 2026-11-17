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
    /// Потолок показа = частота самого потока (30 или 60 fps).
    show_period: Duration,
    last_shown: Option<Instant>,
    /// Конверсия на GPU запущена, кадр ещё не забран в память.
    pending_since: Option<Instant>,
    /// Свежий декодированный кадр, ещё не отправленный в D3D. Держим один:
    /// если кадр пришёл, пока GPU занят, он не теряется, а ждёт следующего
    /// апдейта; более новый просто заменяет его.
    queued: Option<Vec<u8>>,
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
            show_period: Duration::from_secs_f64(1.0 / info.fps.max(1) as f64),
            last_shown: None,
            pending_since: None,
            queued: None,
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

        if !units.is_empty() {
            self.decode_ms = t_decode.elapsed().as_secs_f32() * 1000.0;
        }

        // Короткий кадр (ошибка декодера) нельзя отдавать в D3D: UpdateSubresource
        // прочитает за концом буфера. Лучше пропустить кадр.
        if let Some(nv12) = latest {
            let need = self.width as usize * self.height as usize * 3 / 2;
            if nv12.len() < need {
                self.last_error = Some(format!(
                    "короткий NV12-кадр: {} из {} байт",
                    nv12.len(),
                    need
                ));
            } else {
                self.queued = Some(nv12);
            }
        }

        self.pump(ctx);
    }

    /// Забирает готовый кадр, не дожидаясь GPU (блокирующий `Map` стоит
    /// десятки миллисекунд на кадр — из-за него превью шло ~30 fps).
    pub fn tick(&mut self, ctx: &egui::Context) {
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
        // Показ не чаще частоты потока. Кадр всё равно проходит декодер (иначе
        // порвётся цепочка P-кадров), но в D3D и на экран не идёт.
        //
        // Порог с запасом 25%: при мониторе 75 Гц апдейты идут через 13.3 мс,
        // и жёсткий период 16.7 мс пропускал каждый второй — показ залипал на
        // половине (32-37 fps вместо 60). Один кадр на апдейт всё равно не
        // превышает частоту потока.
        if let Some(last) = self.last_shown {
            if last.elapsed() < self.show_period.mul_f32(0.75) {
                return;
            }
        }

        let Some(nv12) = self.queued.take() else {
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
        self.last_shown = Some(Instant::now());
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
            Ok(None) => return, // GPU ещё считает
            Err(e) => {
                self.pending_since = None;
                self.last_error = Some(e);
                return;
            }
        };
        self.readback_ms = t_read.elapsed().as_secs_f32() * 1000.0;
        self.pending_since = None;

        let t_image = Instant::now();
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

    /// Стоимость этапов последнего кадра: декод, чтение с GPU, картинка+текстура.
    pub fn timings(&self) -> (f32, f32, f32) {
        (self.decode_ms, self.readback_ms, self.image_ms)
    }
}

/// Своя трансляция: захват экрана, превью и кадры для сети.
pub struct VideoSession {
    publisher: ScreenPublisher,
    renderer: Renderer,
    pub source: String,
}

impl VideoSession {
    pub fn start(fps: u32) -> Result<Self, String> {
        let publisher = ScreenPublisher::start(fps)?;
        // превью — в половинном разрешении: на экране оно всё равно мелкое,
        // а полный 1080p readback не даёт держать 60 кадров в секунду
        let renderer = Renderer::new(&publisher.info, 2)?;
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

    pub fn timings(&self) -> (f32, f32, f32) {
        self.renderer.timings()
    }

    /// Фактическая частота кодирования (не показа).
    pub fn encode_fps(&self) -> u32 {
        self.publisher
            .encode_fps
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

    /// Стоимость этапов показа (декод, чтение с GPU, картинка), мс.
    /// Нужна, чтобы по логу/экрану зрителя отличить слабый компьютер
    /// (дорогой декод или чтение с GPU) от потерь в сети.
    pub fn timings(&self) -> (f32, f32, f32) {
        self.renderer.timings()
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
