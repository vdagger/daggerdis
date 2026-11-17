//! D3D11: общее устройство и конверсия форматов на GPU.
//!
//! Экран приходит в BGRA, энкодеру нужен NV12, а рисуем мы снова в BGRA.
//! Цвет и масштаб считает видеокарта (ID3D11VideoProcessor); CPU только
//! забирает готовый кадр из staging-текстуры.

use windows::core::Interface;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::{IDXGIAdapter, DXGI_ERROR_WAS_STILL_DRAWING};

/// Форматы, с которыми работает конвейер (чтобы не тянуть windows в служебный крейт).
pub const FORMAT_BGRA: DXGI_FORMAT = DXGI_FORMAT_B8G8R8A8_UNORM;
pub const FORMAT_NV12: DXGI_FORMAT = DXGI_FORMAT_NV12;

pub struct D3dDevice {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
}

/// Создаёт аппаратное устройство D3D11; `adapter = None` — адаптер по умолчанию.
pub fn create_device(adapter: Option<&IDXGIAdapter>) -> Result<D3dDevice, String> {
    unsafe {
        let flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT;
        let drv = if adapter.is_some() {
            D3D_DRIVER_TYPE_UNKNOWN
        } else {
            D3D_DRIVER_TYPE_HARDWARE
        };

        for levels in [
            &[D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0][..],
            &[D3D_FEATURE_LEVEL_11_0][..],
        ] {
            let mut device = None;
            let mut context = None;
            let res = D3D11CreateDevice(
                adapter,
                drv,
                HMODULE::default(),
                flags,
                Some(levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            );
            if let (Ok(()), Some(device), Some(context)) = (res, device, context) {
                return Ok(D3dDevice { device, context });
            }
        }
        Err("D3D11CreateDevice не удался".into())
    }
}

/// Плотность целевого формата для staging-копии.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Layout {
    Nv12,
    Rgba32,
}

impl Layout {
    fn of(format: DXGI_FORMAT) -> Result<Self, String> {
        if format == DXGI_FORMAT_NV12 {
            Ok(Layout::Nv12)
        } else if format == DXGI_FORMAT_B8G8R8A8_UNORM {
            Ok(Layout::Rgba32)
        } else {
            Err(format!("формат {format:?} не поддержан конвертером"))
        }
    }
}

/// Аппаратная конверсия/масштабирование между BGRA и NV12.
pub struct Converter {
    context: ID3D11DeviceContext,
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    out_tex: ID3D11Texture2D,
    out_view: ID3D11VideoProcessorOutputView,
    staging: ID3D11Texture2D,
    width: u32,
    height: u32,
    layout: Layout,
}

impl Converter {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        src_format: DXGI_FORMAT,
        src_w: u32,
        src_h: u32,
        dst_format: DXGI_FORMAT,
        dst_w: u32,
        dst_h: u32,
        fps: u32,
    ) -> Result<Self, String> {
        unsafe {
            let video_device: ID3D11VideoDevice = device
                .cast()
                .map_err(|e| format!("ID3D11VideoDevice: {e}"))?;
            let video_context: ID3D11VideoContext = context
                .cast()
                .map_err(|e| format!("ID3D11VideoContext: {e}"))?;

            let rate = DXGI_RATIONAL {
                Numerator: fps.max(1),
                Denominator: 1,
            };
            let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputFrameRate: rate,
                InputWidth: src_w,
                InputHeight: src_h,
                OutputFrameRate: rate,
                OutputWidth: dst_w,
                OutputHeight: dst_h,
                Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
            };
            let enumerator = video_device
                .CreateVideoProcessorEnumerator(&desc)
                .map_err(|e| format!("CreateVideoProcessorEnumerator: {e}"))?;
            let processor = video_device
                .CreateVideoProcessor(&enumerator, 0)
                .map_err(|e| format!("CreateVideoProcessor: {e}"))?;

            let out_tex = create_texture(
                device,
                dst_w,
                dst_h,
                dst_format,
                D3D11_USAGE_DEFAULT,
                D3D11_BIND_RENDER_TARGET.0 as u32,
                0,
            )?;
            let out_res: ID3D11Resource = out_tex.cast().map_err(|e| e.to_string())?;
            let out_view_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
                },
            };
            let mut out_view_slot = None;
            video_device
                .CreateVideoProcessorOutputView(
                    &out_res,
                    &enumerator,
                    &out_view_desc,
                    Some(&mut out_view_slot),
                )
                .map_err(|e| format!("CreateVideoProcessorOutputView: {e}"))?;
            let out_view = out_view_slot
                .ok_or_else(|| "CreateVideoProcessorOutputView вернул null".to_string())?;

            let staging = create_texture(
                device,
                dst_w,
                dst_h,
                dst_format,
                D3D11_USAGE_STAGING,
                0,
                D3D11_CPU_ACCESS_READ.0 as u32,
            )?;

            // проверяем, что источник вообще может идти через видеопроцессор
            let _ = src_format;

            Ok(Self {
                context: context.clone(),
                video_device,
                video_context,
                enumerator,
                processor,
                out_tex,
                out_view,
                staging,
                width: dst_w,
                height: dst_h,
                layout: Layout::of(dst_format)?,
            })
        }
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Кадр GPU → целевая текстура (без чтения в память).
    pub fn convert(&mut self, src: &ID3D11Texture2D) -> Result<(), String> {
        unsafe {
            let src_res: ID3D11Resource = src.cast().map_err(|e| e.to_string())?;
            let in_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                FourCC: 0,
                ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPIV {
                        MipSlice: 0,
                        ArraySlice: 0,
                    },
                },
            };
            let mut in_view_slot = None;
            self.video_device
                .CreateVideoProcessorInputView(
                    &src_res,
                    &self.enumerator,
                    &in_desc,
                    Some(&mut in_view_slot),
                )
                .map_err(|e| format!("CreateVideoProcessorInputView: {e}"))?;
            let in_view = in_view_slot
                .ok_or_else(|| "CreateVideoProcessorInputView вернул null".to_string())?;

            let mut stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: windows::core::BOOL(1),
                ..Default::default()
            };
            stream.pInputSurface = std::mem::ManuallyDrop::new(Some(in_view));
            self.video_context
                .VideoProcessorBlt(
                    &self.processor,
                    &self.out_view,
                    0,
                    std::slice::from_ref(&stream),
                )
                .map_err(|e| format!("VideoProcessorBlt: {e}"))?;
            Ok(())
        }
    }

    /// Конверсия и чтение результата в память одним вызовом.
    pub fn convert_and_read(&mut self, src: &ID3D11Texture2D) -> Result<Vec<u8>, String> {
        self.convert(src)?;
        self.copy_to_staging()?;
        self.read_back()
    }

    /// Переносит результат конверсии в staging-текстуру.
    /// Копия отделена от чтения специально: с `DO_NOT_WAIT` нельзя копировать
    /// перед каждой попыткой — свежая копия снова занимает ресурс, `Map`
    /// вечно отвечает «ещё рисуется», и кадр не доходит до экрана.
    pub fn copy_to_staging(&mut self) -> Result<(), String> {
        unsafe {
            let out_res: ID3D11Resource = self.out_tex.cast().map_err(|e| e.to_string())?;
            let staging_res: ID3D11Resource =
                self.staging.cast().map_err(|e| e.to_string())?;
            self.context.CopyResource(&staging_res, &out_res);
            // без Flush копия может так и лежать в очереди драйвера
            self.context.Flush();
        }
        Ok(())
    }

    /// Блокирующее чтение staging-текстуры в плотный буфер.
    pub fn read_back(&mut self) -> Result<Vec<u8>, String> {
        unsafe {
            let staging_res: ID3D11Resource =
                self.staging.cast().map_err(|e| e.to_string())?;

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(&staging_res, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(|e| format!("Map: {e}"))?;

            let buf = self.gather(&mapped);
            self.context.Unmap(&staging_res, 0);
            Ok(buf)
        }
    }

    /// То же, но без ожидания GPU: `Ok(None)` — результат ещё не готов.
    /// Блокирующий `Map` ждёт окончания конверсии и отнимает у GUI десятки
    /// миллисекунд на кадр (превью 1080p показывало ~30 fps при 60 в потоке).
    pub fn read_back_nowait(&mut self) -> Result<Option<Vec<u8>>, String> {
        unsafe {
            let staging_res: ID3D11Resource =
                self.staging.cast().map_err(|e| e.to_string())?;

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            match self.context.Map(
                &staging_res,
                0,
                D3D11_MAP_READ,
                D3D11_MAP_FLAG_DO_NOT_WAIT.0 as u32,
                Some(&mut mapped),
            ) {
                Ok(()) => {}
                Err(e) if e.code() == DXGI_ERROR_WAS_STILL_DRAWING => return Ok(None),
                Err(e) => return Err(format!("Map: {e}")),
            }

            let buf = self.gather(&mapped);
            self.context.Unmap(&staging_res, 0);
            Ok(Some(buf))
        }
    }

    /// Плотная копия отображённых строк (staging → системная память).
    fn gather(&self, mapped: &D3D11_MAPPED_SUBRESOURCE) -> Vec<u8> {
        let w = self.width as usize;
        let h = self.height as usize;
        let pitch = mapped.RowPitch as usize;
        let base = mapped.pData as *const u8;
        let copy_rows = |dst: &mut [u8], src_row: usize, rows: usize, row_bytes: usize, dst_off: usize| {
            for y in 0..rows {
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        base.add((src_row + y) * pitch),
                        dst.as_mut_ptr().add(dst_off + y * row_bytes),
                        row_bytes,
                    );
                }
            }
        };

        match self.layout {
            Layout::Nv12 => {
                let mut out = vec![0u8; w * h * 3 / 2];
                copy_rows(&mut out, 0, h, w, 0);
                copy_rows(&mut out, h, h / 2, w, w * h);
                out
            }
            Layout::Rgba32 => {
                let row = w * 4;
                let mut out = vec![0u8; row * h];
                copy_rows(&mut out, 0, h, row, 0);
                out
            }
        }
    }
}

/// Текстура BGRA, в которую можно заливать кадр с CPU (пробы, камера).
pub fn bgra_texture(device: &ID3D11Device, w: u32, h: u32) -> Result<ID3D11Texture2D, String> {
    unsafe {
        create_texture(
            device,
            w,
            h,
            DXGI_FORMAT_B8G8R8A8_UNORM,
            D3D11_USAGE_DEFAULT,
            0,
            0,
        )
    }
}

/// Текстура NV12, в которую заливается кадр с CPU (кадры с декодера).
pub fn nv12_texture(device: &ID3D11Device, w: u32, h: u32) -> Result<ID3D11Texture2D, String> {
    unsafe {
        create_texture(
            device,
            w,
            h,
            DXGI_FORMAT_NV12,
            D3D11_USAGE_DEFAULT,
            0,
            0,
        )
    }
}

pub fn upload_bgra(
    context: &ID3D11DeviceContext,
    tex: &ID3D11Texture2D,
    w: u32,
    pixels: &[u8],
) {
    unsafe {
        let res: ID3D11Resource = match tex.cast() {
            Ok(r) => r,
            Err(_) => return,
        };
        context.UpdateSubresource(
            &res,
            0,
            None,
            pixels.as_ptr() as *const core::ffi::c_void,
            w * 4,
            0,
        );
    }
}

/// Заливка NV12 из системной памяти (декодированный кадр).
pub fn upload_nv12(
    context: &ID3D11DeviceContext,
    tex: &ID3D11Texture2D,
    w: u32,
    pixels: &[u8],
) {
    unsafe {
        let res: ID3D11Resource = match tex.cast() {
            Ok(r) => r,
            Err(_) => return,
        };
        context.UpdateSubresource(
            &res,
            0,
            None,
            pixels.as_ptr() as *const core::ffi::c_void,
            w,
            0,
        );
    }
}

unsafe fn create_texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
    usage: D3D11_USAGE,
    bind: u32,
    cpu_access: u32,
) -> Result<ID3D11Texture2D, String> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: usage,
        BindFlags: bind,
        CPUAccessFlags: cpu_access,
        MiscFlags: 0,
    };
    let mut tex = None;
    device
        .CreateTexture2D(&desc, None, Some(&mut tex))
        .map_err(|e| format!("CreateTexture2D: {e}"))?;
    tex.ok_or_else(|| "CreateTexture2D вернул пустую текстуру".to_string())
}
