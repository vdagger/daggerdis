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
    fps_window_start: Instant,
    fps_window_frames: u32,
    fps: f32,
    /// Потолок показа = частота самого потока (30 или 60 fps).
    show_period: Duration,
    last_shown: Option<Instant>,
    pub last_error: Option<String>,
}

impl Renderer {
    pub fn new(info: &StreamInfo) -> Result<Self, String> {
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
            info.width,
            info.height,
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
            fps_window_start: Instant::now(),
            fps_window_frames: 0,
            fps: 0.0,
            show_period: Duration::from_secs_f64(1.0 / info.fps.max(1) as f64),
            last_shown: None,
            last_error: None,
        })
    }

    pub fn feed(&mut self, units: &[EncodedUnit], ctx: &egui::Context) {
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

        let Some(nv12) = latest else { return };

        // Показ не чаще частоты потока. Кадр всё равно проходит декодер (иначе
        // порвётся цепочка P-кадров), но в D3D и на экран не идёт.
        if let Some(last) = self.last_shown {
            if last.elapsed() < self.show_period {
                return;
            }
        }
        self.last_shown = Some(Instant::now());

        // Короткий кадр (ошибка декодера) нельзя отдавать в D3D: UpdateSubresource
        // прочитает за концом буфера. Лучше пропустить кадр.
        let need = self.width as usize * self.height as usize * 3 / 2;
        if nv12.len() < need {
            self.last_error = Some(format!(
                "короткий NV12-кадр: {} из {} байт",
                nv12.len(),
                need
            ));
            return;
        }
        upload_nv12(&self.d3d.context, &self.nv12_tex, self.width, &nv12);
        let bgra = match self.converter.convert_and_read(&self.nv12_tex) {
            Ok(b) => b,
            Err(e) => {
                self.last_error = Some(e);
                return;
            }
        };
        let image = bgra_to_image(&bgra, self.width, self.height);
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
        (self.width, self.height)
    }

    /// Кадров в секунду за последнее окно измерения.
    pub fn fps(&self) -> f32 {
        self.fps
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
        let renderer = Renderer::new(&publisher.info)?;
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
            renderer: Renderer::new(info)?,
            peer,
        })
    }

    pub fn feed(&mut self, unit: EncodedUnit, ctx: &egui::Context) {
        self.renderer.feed(std::slice::from_ref(&unit), ctx);
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
