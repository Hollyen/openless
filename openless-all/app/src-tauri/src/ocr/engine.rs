//! OCR 引擎公共 trait 与类型。

use serde::{Deserialize, Serialize};

/// 单行 OCR 结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrLine {
    pub text: String,
    pub confidence: Option<f32>,
}

/// OCR 整图结果。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrResult {
    pub lines: Vec<OcrLine>,
}

/// 模型下载状态。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadStatus {
    pub provider_id: String,
    pub model_id: String,
    pub state: DownloadState,
    pub bytes_total: Option<u64>,
    pub bytes_downloaded: Option<u64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DownloadState {
    #[default]
    NotStarted,
    InProgress,
    Completed,
    Failed,
    Cancelled,
}

/// 可切换的 OCR 提供方。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OcrProvider {
    pub id: String,
    pub name: String,
    pub description: String,
    pub local: bool,
    pub requires_download: bool,
    pub supported_platforms: Vec<String>,
}

/// OCR 引擎能力接口。
/// 所有实现必须是 `Send + Sync`，以便在 `tokio::task::spawn_blocking` 中运行。
pub trait OcrEngine: Send + Sync {
    /// provider id，如 `winrt`、`rapidocr`。
    fn provider_id(&self) -> &str;

    /// 对单张图片（PNG/JPEG 字节）执行 OCR，返回按阅读顺序排列的行。
    fn recognize(&self, image_bytes: &[u8]) -> anyhow::Result<OcrResult>;

    /// 是否需要下载模型才能工作。
    fn requires_download(&self) -> bool;

    /// 当前下载状态（仅对需要下载的引擎有意义）。返回多个文件的进度列表。
    fn download_status(&self) -> Option<Vec<DownloadStatus>> {
        None
    }

    /// 启动/确保模型下载完成。默认空实现。
    fn ensure_model_downloaded(&self) -> anyhow::Result<()> {
        Ok(())
    }

    /// 取消下载。默认空实现。
    fn cancel_download(&self) {}

    /// 删除已下载的模型以释放空间。默认空实现。
    fn delete_model(&self) -> anyhow::Result<()> {
        Ok(())
    }
}
