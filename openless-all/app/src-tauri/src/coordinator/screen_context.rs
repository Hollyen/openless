//! 屏幕上下文捕获：截图 + OCR，为润色模块提供当前屏幕可见文本。
//!
//! 实现分层：
//! - `ocr` 模块负责引擎抽象、截图、识别。
//! - 本模块只负责根据 `UserPreferences` 触发一次调用，并做超时/降级保护。
//!
//! 超时：OCR 与截图不应阻塞 dictation pipeline。若 5 秒内未返回，则视为失败，
//! 返回 `None`，让润色链路继续走无屏幕上下文路径。

use std::sync::Arc;
use std::time::Duration;

use super::Inner;

/// 为一次润色请求捕获屏幕上下文文本。
///
/// 当前默认超时 5 秒；截图或 OCR 失败均返回 `None`，保证 dictation 可用性。
pub(super) async fn capture_screen_context(inner: &Arc<Inner>) -> Option<String> {
    let prefs = inner.prefs.get();
    if !prefs.screen_context_enabled {
        return None;
    }

    match tokio::time::timeout(
        Duration::from_secs(5),
        crate::ocr::recognize_screen_text(&prefs),
    )
    .await
    {
        Ok(Ok(Some(text))) => {
            let chars = text.chars().count();
            log::info!("[screen_context] captured {chars} chars");
            Some(text)
        }
        Ok(Ok(None)) => {
            log::info!("[screen_context] no screen text captured");
            None
        }
        Ok(Err(e)) => {
            log::warn!("[screen_context] OCR failed: {e:#}");
            None
        }
        Err(_) => {
            log::warn!("[screen_context] OCR timed out after 5s");
            None
        }
    }
}

/// 同步变体，仅用于测试或需要同步调用的场景。
#[allow(dead_code)]
pub(super) fn capture_screen_context_sync() -> Option<String> {
    None
}
