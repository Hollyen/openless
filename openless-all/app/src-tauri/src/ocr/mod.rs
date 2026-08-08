//! 屏幕 OCR 后端抽象与实现入口。
//!
//! 设计目标：与 ASR/LLM 提供方一样，支持多个 OCR 引擎、可切换、可管理模型下载。
//! 当前实现：
//! - `disabled`：关闭屏幕上下文
//! - `winrt`：Windows 10/11 原生 `Windows.Media.Ocr`（零模型下载）
//! - `rapidocr`：RapidOCR / PP-OCRv6 ONNX 管线（rapidocr-core + ort load-dynamic）
//!
//! 调用入口：`recognize_screen_text`，由 `coordinator/screen_context.rs` 在润色前调用。

use crate::types::UserPreferences;

pub mod disabled;
pub mod engine;
pub mod rapidocr;
#[cfg(target_os = "windows")]
pub mod winrt;

pub use engine::{DownloadState, DownloadStatus, OcrEngine, OcrLine, OcrProvider, OcrResult};

/// 根据用户偏好构造当前激活的 OCR 引擎。
/// 当屏幕上下文关闭或未知 provider 时返回 disabled 引擎。
pub fn active_engine(prefs: &UserPreferences) -> Box<dyn OcrEngine + Send + Sync> {
    if !prefs.screen_context_enabled {
        return Box::new(disabled::DisabledOcrEngine);
    }
    match prefs.active_ocr_provider.as_str() {
        #[cfg(target_os = "windows")]
        "winrt" => Box::new(winrt::WinRtOcrEngine::new()),
        "rapidocr" => Box::new(rapidocr::RapidOcrEngine::new(prefs)),
        // 未知/未配置：回退到关闭
        _ => {
            log::warn!(
                "[ocr] unknown active provider '{}', falling back to disabled",
                prefs.active_ocr_provider
            );
            Box::new(disabled::DisabledOcrEngine)
        }
    }
}

/// 列出所有可用的 OCR 提供方（前端「设置 → 服务」下拉用）。
pub fn available_providers() -> Vec<OcrProvider> {
    let mut providers = vec![OcrProvider {
        id: "disabled".into(),
        name: "关闭".into(),
        description: "不注入屏幕上下文".into(),
        local: true,
        requires_download: false,
        supported_platforms: vec!["windows".into(), "macos".into(), "linux".into()],
    }];
    providers.extend(rapidocr::RapidOcrEngine::provider_list());
    #[cfg(target_os = "windows")]
    providers.extend(winrt::WinRtOcrEngine::provider_list());
    providers
}

/// 捕获屏幕并执行 OCR，返回合并后的文本（已按阅读顺序拼接）。
/// 这是 `coordinator/screen_context.rs` 的唯一外部入口。
pub async fn recognize_screen_text(prefs: &UserPreferences) -> anyhow::Result<Option<String>> {
    let engine = active_engine(prefs);

    // disabled 引擎直接短路，避免截图/编码开销。
    if engine.provider_id() == "disabled" {
        return Ok(None);
    }

    // 确保需要下载的引擎已完成模型下载。
    if engine.requires_download() {
        engine.ensure_model_downloaded()?;
    }

    // 1. 捕获屏幕：目前 Windows/macOS 支持全屏主屏截图；后续可切换为前台窗口/光标区域。
    let capture = tokio::task::spawn_blocking({
        let prefs = prefs.clone();
        move || capture_primary_screen(&prefs)
    })
    .await
    .map_err(|e| anyhow::anyhow!("capture task panicked: {e}"))?;

    let Some(image_bytes) = capture? else {
        return Ok(None);
    };

    // 2. 跑 OCR。WinRT/OCR 引擎内部可能依赖 COM/STA，用 spawn_blocking 避免阻塞 async runtime。
    let result = tokio::task::spawn_blocking(move || engine.recognize(&image_bytes))
        .await
        .map_err(|e| anyhow::anyhow!("ocr task panicked: {e}"))?;

    let result = result?;
    if result.lines.is_empty() {
        return Ok(None);
    }

    let text = result
        .lines
        .into_iter()
        .map(|line| line.text)
        .collect::<Vec<_>>()
        .join("\n");

    Ok(Some(text))
}

/// 主屏幕截图 → PNG 字节。
/// 失败时记录日志并返回 None（优雅降级）。
///
/// macOS 屏幕录制（TCC）权限未开启时，`CGWindowListCreateImage` 不会报错，只会返回
/// 空白图像，因此必须先通过 `CGPreflightScreenCaptureAccess` 主动预检；未授权时返回
/// 带 `screenRecordingDenied` 标记的错误，供设置页展示引导（前端据此显示"打开系统设置"按钮）。
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn capture_primary_screen(_prefs: &UserPreferences) -> anyhow::Result<Option<Vec<u8>>> {
    #[cfg(target_os = "macos")]
    {
        use core_graphics::access::ScreenCaptureAccess;
        if !ScreenCaptureAccess::default().preflight() {
            return Err(anyhow::anyhow!(
                "screenRecordingDenied: macOS screen recording permission is not granted"
            ));
        }
    }

    let monitors = xcap::Monitor::all().map_err(|e| anyhow::anyhow!("list monitors: {e}"))?;
    let primary = monitors
        .into_iter()
        .find(|m| m.is_primary().unwrap_or(false))
        .or_else(|| {
            log::warn!("[ocr] no primary monitor found, using first available");
            xcap::Monitor::from_point(0, 0).ok()
        })
        .ok_or_else(|| anyhow::anyhow!("no monitor available"))?;

    let image = primary.capture_image().map_err(|e| {
        // 兜底：个别版本/平台未授权时截图调用本身也会抛错（如 Windows 远程桌面会话、
        // macOS 直接调用被系统拦截），识别常见权限特征并归入 screenRecordingDenied。
        let msg = format!("{e:#}");
        if msg.contains("screen recording")
            || msg.contains("not authorized")
            || msg.contains("CGDisplayCreateImage")
            || msg.contains("kCGError")
            || msg.contains("access denied")
        {
            anyhow::anyhow!("screenRecordingDenied: {msg}")
        } else {
            anyhow::anyhow!("capture primary screen: {msg}")
        }
    })?;

    let mut buf = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
        .map_err(|e| anyhow::anyhow!("encode screenshot to PNG: {e}"))?;

    Ok(Some(buf))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn capture_primary_screen(_prefs: &UserPreferences) -> anyhow::Result<Option<Vec<u8>>> {
    log::warn!("[ocr] screen capture not yet implemented on this platform");
    Ok(None)
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::timeout;

    /// 真机冒烟测试：使用 RapidOCR 捕获主屏并识别文字。
    /// 需要已下载模型且默认模型目录存在 onnxruntime.dll。
    /// 屏幕录制权限未开启时允许返回错误/空结果。
    #[tokio::test]
    #[ignore = "requires screen capture permission and downloaded RapidOCR models"]
    async fn rapidocr_screen_context_smoke_test() {
        let mut prefs = UserPreferences::default();
        prefs.screen_context_enabled = true;
        prefs.active_ocr_provider = "rapidocr".into();

        let result = timeout(Duration::from_secs(120), recognize_screen_text(&prefs)).await;
        match result {
            Ok(Ok(Some(text))) => {
                eprintln!("[smoke] captured {} chars", text.len());
                eprintln!("[smoke] preview: {}", &text[..text.len().min(200)]);
                assert!(!text.is_empty(), "OCR returned empty text");
            }
            Ok(Ok(None)) => {
                eprintln!("[smoke] no OCR text (blank screen or permission not granted)");
            }
            Ok(Err(e)) => {
                eprintln!("[smoke] OCR error (acceptable if permission missing): {e:#}");
            }
            Err(_) => panic!("screen OCR test timed out after 120s"),
        }
    }
}

