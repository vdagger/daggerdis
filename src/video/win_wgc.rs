//! Захват одного окна через Windows Graphics Capture.
//!
//! В отличие от вырезания прямоугольника из кадра рабочего стола, WGC отдаёт
//! кадры самого окна: его можно перекрыть другим окном, и в кадр всё равно
//! попадёт только оно. Прямоугольник в `Converter` при этом не нужен: текстура
//! кадра уже размером с окно.

// windows 0.62 генерирует безопасные обёртки почти для всех вызовов WinRT,
// поэтому часть `unsafe` в этом модуле лишняя; имена методов COM — как в
// заголовках (`CreateForWindow`), переименовывать их нельзя.
#![allow(unused_unsafe, non_snake_case)]

use std::time::{Duration, Instant};

use windows::core::{factory, Interface};
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows_core::{GUID, HRESULT, IUnknown};
// Тип нужен макросу `interface`: он строит vtable поверх IUnknown.
use windows_core::IUnknown_Vtbl;

use super::win_d3d::{create_device, Converter};

/// `IGraphicsCaptureItemInterop` — Win32-интерфейс, которого нет в биндингах
/// windows 0.62: объявляем сами, ABI простой (два метода).
#[windows::core::interface("3628E81B-3CAC-4C60-B7F4-23CE0E0C3356")]
unsafe trait IGraphicsCaptureItemInterop: IUnknown {
    unsafe fn CreateForWindow(
        &self,
        window: HWND,
        riid: *const GUID,
        result: *mut *mut core::ffi::c_void,
    ) -> HRESULT;
    unsafe fn CreateForMonitor(
        &self,
        monitor: isize,
        riid: *const GUID,
        result: *mut *mut core::ffi::c_void,
    ) -> HRESULT;
}

/// Создаёт item окна для WGC. `None` — система не даёт захват этого окна.
fn item_for_window(hwnd: isize) -> Option<GraphicsCaptureItem> {
    let interop: IGraphicsCaptureItemInterop =
        factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().ok()?;
    let mut raw: *mut core::ffi::c_void = std::ptr::null_mut();
    unsafe {
        let created =
            interop.CreateForWindow(HWND(hwnd as *mut _), &GraphicsCaptureItem::IID, &mut raw);
        if created.is_err() || raw.is_null() {
            return None;
        }
        Some(GraphicsCaptureItem::from_raw(raw))
    }
}

/// Доступен ли WGC на этой системе (Windows 10 1903+ и не в защищённом режиме).
pub fn supported() -> bool {
    unsafe { GraphicsCaptureSession::IsSupported().unwrap_or(false) }
}

/// Захват одного окна: кадры приходят уже размером с окно и не зависят от
/// перекрытия.
pub struct WindowCapture {
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    converter: Converter,
    width: u32,
    height: u32,
    pub adapter_name: String,
}

impl WindowCapture {
    pub fn new(hwnd: isize, fps: u32) -> Result<Self, String> {
        if !supported() {
            return Err("Windows Graphics Capture недоступен".into());
        }
        unsafe {
            // COM нужен потоку публикации: WinRT-объекты живут в нём.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let d3d = create_device(None)?;
        let dxgi: IDXGIDevice = d3d.device.cast().map_err(|e| e.to_string())?;
        let device: IDirect3DDevice = unsafe {
            CreateDirect3D11DeviceFromDXGIDevice(&dxgi)
                .map_err(|e| format!("CreateDirect3D11DeviceFromDXGIDevice: {e}"))?
                .cast()
                .map_err(|e| format!("IDirect3DDevice: {e}"))?
        };
        let item = item_for_window(hwnd).ok_or_else(|| "окно не открылось для захвата".to_string())?;
        let size = unsafe { item.Size() }.map_err(|e| format!("item.Size: {e}"))?;
        // NV12 и H.264 требуют чётных размеров; окно может быть нечётным.
        let width = ((size.Width.max(2) as u32) & !1).max(2);
        let height = ((size.Height.max(2) as u32) & !1).max(2);
        let pool = unsafe {
            Direct3D11CaptureFramePool::CreateFreeThreaded(
                &device,
                DirectXPixelFormat::B8G8R8A8UIntNormalized,
                2,
                SizeInt32 {
                    Width: width as i32,
                    Height: height as i32,
                },
            )
            .map_err(|e| format!("CreateFreeThreaded: {e}"))?
        };
        let session = unsafe { pool.CreateCaptureSession(&item) }
            .map_err(|e| format!("CreateCaptureSession: {e}"))?;
        let _ = unsafe { session.SetIsCursorCaptureEnabled(true) };
        unsafe { session.StartCapture() }.map_err(|e| format!("StartCapture: {e}"))?;
        let converter = Converter::new(
            &d3d.device,
            &d3d.context,
            DXGI_FORMAT_B8G8R8A8_UNORM,
            width,
            height,
            DXGI_FORMAT_NV12,
            width,
            height,
            fps,
        )?;
        println!("[ВИДЕО] Захват окна через WGC: {width}x{height}");
        Ok(Self {
            pool,
            session,
            converter,
            width,
            height,
            adapter_name: "WGC".to_string(),
        })
    }

    /// Ждёт следующий кадр окна и отдаёт его в NV12. `Ok(None)` — окно не
    /// менялось за отведённое время.
    pub fn next_nv12(&mut self, timeout_ms: u32) -> Result<Option<Vec<u8>>, String> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms as u64);
        loop {
            match unsafe { self.pool.TryGetNextFrame() } {
                Ok(frame) => {
                    let converted = (|| {
                        let surface = unsafe { frame.Surface() }.map_err(|e| e.to_string())?;
                        let access: IDirect3DDxgiInterfaceAccess =
                            surface.cast().map_err(|e| e.to_string())?;
                        let texture: ID3D11Texture2D = unsafe { access.GetInterface() }
                            .map_err(|e| e.to_string())?;
                        self.converter.convert_and_read(&texture)
                    })();
                    let _ = unsafe { frame.Close() };
                    // Проверка размера: конвертер обязан отдать плотный NV12
                    // ровно w×h. Иначе кадр будет мусором на экране, и лучше
                    // сказать это словами, чем показать полосы.
                    let expected = (self.width * self.height * 3 / 2) as usize;
                    return match converted {
                        Ok(buf) if buf.len() == expected => Ok(Some(buf)),
                        Ok(buf) => Err(format!(
                            "WGC: кадр {} байт вместо {} ({}x{})",
                            buf.len(),
                            expected,
                            self.width,
                            self.height
                        )),
                        Err(e) => Err(e),
                    };
                }
                Err(_) => {
                    if Instant::now() >= deadline {
                        return Ok(None);
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        }
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

impl Drop for WindowCapture {
    fn drop(&mut self) {
        let _ = unsafe { self.session.Close() };
        let _ = unsafe { self.pool.Close() };
    }
}
