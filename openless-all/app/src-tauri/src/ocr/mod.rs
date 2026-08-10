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

    // 1. 捕获屏幕：优先只捕获前台窗口，减少像素量和无关噪声；失败时回退全屏主屏。
    let capture_started = std::time::Instant::now();
    let capture = tokio::task::spawn_blocking({
        let prefs = prefs.clone();
        move || capture_foreground_window(&prefs)
    })
    .await
    .map_err(|e| anyhow::anyhow!("capture task panicked: {e}"))?;
    let capture_ms = capture_started.elapsed().as_millis() as u64;

    let Some(image_bytes) = capture? else {
        log::info!("[ocr] capture returned no image after {capture_ms}ms");
        return Ok(None);
    };
    let image_size = image_bytes.len();

    // 2. 跑 OCR。WinRT/OCR 引擎内部可能依赖 COM/STA，用 spawn_blocking 避免阻塞 async runtime。
    let recognize_started = std::time::Instant::now();
    let image_bytes_for_debug = image_bytes.clone();
    let result = tokio::task::spawn_blocking(move || engine.recognize(&image_bytes))
        .await
        .map_err(|e| anyhow::anyhow!("ocr task panicked: {e}"))?;
    let recognize_ms = recognize_started.elapsed().as_millis() as u64;

    let result = result?;
    let raw_count = result.lines.len();
    let cleaned_lines = clean_ocr_lines(result.lines);
    let cleaned_count = cleaned_lines.len();
    log::info!(
        "[ocr] capture={capture_ms}ms image_bytes={image_size} recognize={recognize_ms}ms raw_lines={raw_count} cleaned_lines={cleaned_count}",
    );
    if cleaned_lines.is_empty() {
        return Ok(None);
    }

    let text = cleaned_lines
        .into_iter()
        .map(|line| line.text)
        .collect::<Vec<_>>()
        .join("\n");

    maybe_save_ocr_debug(&image_bytes_for_debug, &text);

    Ok(Some(text))
}

/// 后处理：过滤噪声行（图标/emoji/纯符号）并清理 CJK 字间空格。
fn clean_ocr_lines(lines: Vec<engine::OcrLine>) -> Vec<engine::OcrLine> {
    lines
        .into_iter()
        .filter_map(|line| {
            let text = clean_ocr_text(&line.text)?;
            Some(engine::OcrLine {
                text,
                confidence: line.confidence,
            })
        })
        .collect()
}

fn clean_ocr_text(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    // 过滤 mostly-noise 行（头像、图标、emoji、分割线等）。
    if is_mostly_noise(trimmed) {
        return None;
    }
    // 过滤常见 IM 时间戳/系统提示行。
    if is_likely_timestamp_or_meta(trimmed) {
        return None;
    }
    // 去掉 CJK 字符之间的空格（OCR 偶尔把「你 好」拆成「你 好」）。
    Some(remove_spaces_between_cjk(trimmed))
}

/// 判断字符串是否主要由无意义符号/emoji 组成。
fn is_mostly_noise(text: &str) -> bool {
    let total = text.chars().count();
    if total == 0 {
        return true;
    }
    let significant = text.chars().filter(|c| is_significant_char(*c)).count();
    // 有效字符不足一半视为噪声。
    significant * 2 < total
}

/// 有效字符：字母数字、CJK 统一表意文字、平假名、片假名、韩文音节。
fn is_significant_char(c: char) -> bool {
    c.is_alphanumeric()
        || ('\u{4E00}'..='\u{9FFF}').contains(&c)
        || ('\u{3040}'..='\u{309F}').contains(&c)
        || ('\u{30A0}'..='\u{30FF}').contains(&c)
        || ('\u{AC00}'..='\u{D7AF}').contains(&c)
}

/// 简单启发式过滤 IM 中常见的时间戳/状态行。
fn is_likely_timestamp_or_meta(text: &str) -> bool {
    // 纯时间，如 "10:30"、"14:20:05"
    if text
        .chars()
        .all(|c| c.is_ascii_digit() || c == ':' || c == '-' || c.is_whitespace())
        && text.chars().filter(|c| c.is_ascii_digit()).count() >= 3
    {
        return true;
    }
    // 日期，如 "2024-08-10"、"8/10"
    if text
        .chars()
        .all(|c| c.is_ascii_digit() || c == '-' || c == '/' || c == '.' || c == '年' || c == '月' || c == '日')
        && text.chars().filter(|c| c.is_ascii_digit()).count() >= 4
    {
        return true;
    }
    // 昨天/今天/上午/下午 + 时间
    if (text.starts_with("昨天") || text.starts_with("今天") || text.starts_with("上午") || text.starts_with("下午"))
        && text.chars().any(|c| c.is_ascii_digit())
    {
        return true;
    }
    false
}

/// 移除两个 CJK 字符之间的空格。
fn remove_spaces_between_cjk(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (i, c) in chars.iter().enumerate() {
        if *c == ' '
            && i > 0
            && i + 1 < chars.len()
            && is_cjk(chars[i - 1])
            && is_cjk(chars[i + 1])
        {
            continue;
        }
        out.push(*c);
    }
    out
}

fn is_cjk(c: char) -> bool {
    ('\u{4E00}'..='\u{9FFF}').contains(&c)
}

/// 调试辅助：设置环境变量 OPENLESS_OCR_DEBUG=1 时，把截图和最终 OCR 文本保存到
/// %LOCALAPPDATA%\OpenLess\ocr_debug\（Windows）或对应平台日志目录同级，方便人工核对。
fn maybe_save_ocr_debug(image_bytes: &[u8], text: &str) {
    if std::env::var("OPENLESS_OCR_DEBUG").ok().as_deref() != Some("1") {
        return;
    }
    let base = crate::log_dir_path()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| crate::log_dir_path());
    let dir = base.join("ocr_debug");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!("[ocr] create debug dir failed: {e}");
        return;
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let img_path = dir.join(format!("{ts}_capture.png"));
    let txt_path = dir.join(format!("{ts}_ocr.txt"));
    if let Err(e) = std::fs::write(&img_path, image_bytes) {
        log::warn!("[ocr] write debug image failed: {e}");
    }
    if let Err(e) = std::fs::write(&txt_path, text) {
        log::warn!("[ocr] write debug text failed: {e}");
    }
    log::info!("[ocr] debug saved: {}, {}", img_path.display(), txt_path.display());
}

/// 将截图降采样到最大 1920px 宽度，在保留 IM 小字可读性与控制耗时之间取平衡。
/// 保持宽高比，使用 Triangle 滤波（速度与质量均衡）。
fn downsample_for_ocr(image: image::DynamicImage) -> image::DynamicImage {
    const MAX_WIDTH: u32 = 1920;
    let (w, h) = (image.width(), image.height());
    if w <= MAX_WIDTH {
        return image;
    }
    let scale = MAX_WIDTH as f32 / w as f32;
    let new_h = (h as f32 * scale).round() as u32;
    log::info!("[ocr] downsample screenshot {}x{} -> {}x{}", w, h, MAX_WIDTH, new_h);
    image.resize(MAX_WIDTH, new_h, image::imageops::FilterType::Triangle)
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

    let image = downsample_for_ocr(image.into());

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

/// 捕获前台窗口 → PNG 字节；失败时回退到 `capture_primary_screen`。
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn capture_foreground_window(prefs: &UserPreferences) -> anyhow::Result<Option<Vec<u8>>> {
    match try_capture_foreground_window(prefs) {
        Ok(Some(bytes)) => Ok(Some(bytes)),
        Ok(None) => {
            log::warn!(
                "[ocr] foreground window capture returned None, falling back to primary screen"
            );
            capture_primary_screen(prefs)
        }
        Err(e) => {
            let msg = format!("{e:#}");
            if msg.contains("screenRecordingDenied") {
                return Err(e);
            }
            log::warn!(
                "[ocr] foreground window capture failed: {msg}, falling back to primary screen"
            );
            capture_primary_screen(prefs)
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn capture_foreground_window(prefs: &UserPreferences) -> anyhow::Result<Option<Vec<u8>>> {
    capture_primary_screen(prefs)
}

/// 仅尝试捕获前台窗口，失败/找不到时返回 None 或 Err。
#[cfg(target_os = "windows")]
fn try_capture_foreground_window(_prefs: &UserPreferences) -> anyhow::Result<Option<Vec<u8>>> {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    use xcap::Window;

    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0 == std::ptr::null_mut() {
        log::warn!("[ocr] GetForegroundWindow returned null");
        return Ok(None);
    }
    let hwnd_id = hwnd.0 as usize as u32;

    let windows = Window::all().map_err(|e| anyhow::anyhow!("list windows: {e}"))?;
    let foreground = windows.into_iter().find(|w| {
        w.id().map(|id| id == hwnd_id).unwrap_or(false)
    });

    let Some(window) = foreground else {
        log::warn!("[ocr] foreground window {hwnd_id} not found in xcap window list");
        return Ok(None);
    };

    let image = window.capture_image().map_err(|e| {
        let msg = format!("{e:#}");
        if msg.contains("screen recording")
            || msg.contains("not authorized")
            || msg.contains("access denied")
        {
            anyhow::anyhow!("screenRecordingDenied: {msg}")
        } else {
            anyhow::anyhow!("capture foreground window: {msg}")
        }
    })?;

    let image = downsample_for_ocr(image.into());

    let mut buf = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
        .map_err(|e| anyhow::anyhow!("encode foreground window to PNG: {e}"))?;
    Ok(Some(buf))
}

/// macOS：xcap `Window::all()` 内部使用 `CGWindowListCopyWindowInfo` 枚举窗口（按 Z 序
/// 从顶层到最底层），第一个可见窗口即前台窗口；`capture_image()` 内部使用
/// `CGWindowListCreateImage` 只捕获该窗口。失败时回退到主屏。
#[cfg(target_os = "macos")]
fn try_capture_foreground_window(_prefs: &UserPreferences) -> anyhow::Result<Option<Vec<u8>>> {
    use xcap::Window;

    let windows = Window::all().map_err(|e| anyhow::anyhow!("list windows: {e}"))?;
    let foreground = windows.into_iter().next();

    let Some(window) = foreground else {
        log::warn!("[ocr] no windows found");
        return Ok(None);
    };

    let image = window.capture_image().map_err(|e| {
        let msg = format!("{e:#}");
        if msg.contains("screen recording")
            || msg.contains("not authorized")
            || msg.contains("CGDisplayCreateImage")
            || msg.contains("kCGError")
            || msg.contains("access denied")
        {
            anyhow::anyhow!("screenRecordingDenied: {msg}")
        } else {
            anyhow::anyhow!("capture foreground window: {msg}")
        }
    })?;

    let image = downsample_for_ocr(image.into());

    let mut buf = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
        .map_err(|e| anyhow::anyhow!("encode foreground window to PNG: {e}"))?;
    Ok(Some(buf))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn try_capture_foreground_window(_prefs: &UserPreferences) -> anyhow::Result<Option<Vec<u8>>> {
    Ok(None)
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::timeout;

    #[test]
    fn foreground_window_capture_failure_fallback_does_not_panic() {
        // 测试环境可能没有真实前台窗口或截图失败，前台窗口捕获应回退到主屏且不 panic。
        let result = capture_foreground_window(&UserPreferences::default());
        assert!(
            result.is_ok(),
            "前台窗口捕获失败时回退逻辑不应 panic: {:?}",
            result.err()
        );
    }

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

