//! 屏幕上下文关闭时的 OCR 引擎占位实现。

use super::engine::{OcrEngine, OcrResult};

pub struct DisabledOcrEngine;

impl OcrEngine for DisabledOcrEngine {
    fn provider_id(&self) -> &str {
        "disabled"
    }

    fn recognize(&self, _image_bytes: &[u8]) -> anyhow::Result<OcrResult> {
        Ok(OcrResult::default())
    }

    fn requires_download(&self) -> bool {
        false
    }
}
