//! RapidOCR / PP-OCRv6 ONNX 引擎（rapidocr-core + ort load-dynamic）。
//!
//! 模型文件清单（由 `start_download` 下载到模型目录）：
//! - `PP-OCRv6_det_small.onnx`  文本检测
//! - `ch_PP-OCRv6_rec_small.onnx`  中文识别（v6，GitHub release 文件名带 `ch_` 前缀）
//! - `ch_ppocr_mobile_v2.0_cls_mobile.onnx`  方向分类
//! - `ppocrv6_dict.txt`  识别字典（rec 阶段必需，rapidocr-core 的 rec 模型不内嵌 vocab）
//!
//! 推理管线：det → cls → rec，复用 rapidocr-core 0.2.2 的 `RapidOcr` runner。
//! ort 以 `load-dynamic` 特性运行：首次 `recognize` 时通过 `ort::init_from`
//! 加载独立的 ONNX Runtime 动态库，避免与 foundry-local-sdk 的 onnxruntime.dll 冲突。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use once_cell::sync::Lazy;

use super::engine::{DownloadState, DownloadStatus, OcrEngine, OcrResult};
use crate::types::UserPreferences;

// rapidocr-core / ort 只在桌面目标段声明（见 Cargo.toml），其余平台（Linux/移动端）
// 不编译本模块的推理代码，recognize 直接返回平台不支持错误。
#[cfg(any(target_os = "macos", target_os = "windows"))]
use rapidocr_core::{config::RapidOcrConfig, RapidOcr};

/// 默认 RapidOCR 模型集（small，适合 CPU）。
/// 前 3 个是 ONNX 模型，第 4 个是 rec 阶段必需的字典文件。
/// 模型托管在 ModelScope（国内可访问），文件路径/文件名与 rapidocr-core
/// 0.2.2 的 `ppocr_v6_small` 注册表对齐；rec 文件名保持为
/// `PP-OCRv6_rec_small.onnx`（ModelScope 原始文件名），因此 build_config 不再做额外 rename。
const RAPIDOCR_MODEL_SET: &[(&str, &str)] = &[
    (
        "PP-OCRv6_det_small.onnx",
        "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2/onnx/PP-OCRv6/det/PP-OCRv6_det_small.onnx",
    ),
    (
        "PP-OCRv6_rec_small.onnx",
        "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2/onnx/PP-OCRv6/rec/PP-OCRv6_rec_small.onnx",
    ),
    (
        "ch_ppocr_mobile_v2.0_cls_mobile.onnx",
        "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2/onnx/PP-OCRv4/cls/ch_ppocr_mobile_v2.0_cls_mobile.onnx",
    ),
    (
        "ppocrv6_dict.txt",
        "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2/paddle/PP-OCRv6/rec/PP-OCRv6_rec_small/ppocrv6_dict.txt",
    ),
];

const PROVIDER_ID: &str = "rapidocr";

/// 全局下载状态表：model_id → DownloadStatus。
static DOWNLOADS: Lazy<Mutex<HashMap<String, DownloadStatus>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

pub struct RapidOcrEngine {
    models_dir: PathBuf,
    /// 缓存的推理管线（det/cls/rec 三个 ONNX session）。`RapidOcr` 的推理方法
    /// 需要 `&mut self`，用 Mutex 包一层以保持 `OcrEngine: Send + Sync`。
    /// 仅桌面平台持有（rapidocr-core 只在对应目标段编译）。
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    pipeline: Mutex<Option<RapidOcr>>,
}

impl RapidOcrEngine {
    /// 根据偏好计算模型目录：`ocr_models_base_dir`（空 → 默认模型根目录）下的
    /// `rapidocr/` 子目录。命令层展示实际路径时也用同一函数，避免两处不一致。
    pub fn models_dir_for(prefs: &UserPreferences) -> PathBuf {
        let base = if prefs.ocr_models_base_dir.is_empty() {
            crate::persistence::default_models_root().unwrap_or_else(|_| std::env::temp_dir())
        } else {
            PathBuf::from(&prefs.ocr_models_base_dir)
        };
        base.join("rapidocr")
    }

    pub fn new(prefs: &UserPreferences) -> Self {
        Self {
            models_dir: Self::models_dir_for(prefs),
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            pipeline: Mutex::new(None),
        }
    }

    fn model_file_path(&self, filename: &str) -> PathBuf {
        self.models_dir.join(filename)
    }

    fn all_model_files_exist(&self) -> bool {
        RAPIDOCR_MODEL_SET
            .iter()
            .all(|(filename, _)| self.model_file_path(filename).exists())
    }

    pub fn start_download(&self, mirror: &str) {
        let models_dir = self.models_dir.clone();
        for (filename, url) in RAPIDOCR_MODEL_SET {
            let model_id = format!("{PROVIDER_ID}/{filename}");
            let download_url = model_download_url(mirror, filename, url);
            let status = DownloadStatus {
                provider_id: PROVIDER_ID.into(),
                model_id: model_id.clone(),
                state: DownloadState::InProgress,
                ..Default::default()
            };
            DOWNLOADS.lock().unwrap().insert(model_id.clone(), status);

            let models_dir = models_dir.clone();
            tokio::spawn(async move {
                let result = download_one(&model_id, &download_url, &models_dir).await;
                DOWNLOADS
                    .lock()
                    .unwrap()
                    .insert(model_id.clone(), result.clone());
                match &result.state {
                    DownloadState::Completed => {
                        log::info!("[ocr] {model_id} download completed")
                    }
                    _ => log::error!(
                        "[ocr] {model_id} download failed: {:?}",
                        result.error
                    ),
                }
            });
        }
    }

    pub fn delete_models(&self) -> anyhow::Result<()> {
        if self.models_dir.exists() {
            std::fs::remove_dir_all(&self.models_dir)?;
        }
        DOWNLOADS.lock().unwrap().clear();
        self.reset_pipeline();
        Ok(())
    }

    /// 丢弃已加载的 ONNX session，避免句柄/内存残留。
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn reset_pipeline(&self) {
        *self.pipeline.lock().unwrap() = None;
    }

    /// 非桌面平台无 pipeline 字段，空实现。
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn reset_pipeline(&self) {}

    pub fn provider_list() -> Vec<super::engine::OcrProvider> {
        vec![super::engine::OcrProvider {
            id: PROVIDER_ID.into(),
            name: "RapidOCR (PP-OCRv6)".into(),
            description: "本地 ONNX 模型，中文识别质量好，需下载约 30 MB 模型".into(),
            local: true,
            requires_download: true,
            supported_platforms: vec!["windows".into(), "macos".into(), "linux".into()],
        }]
    }
}

impl OcrEngine for RapidOcrEngine {
    fn provider_id(&self) -> &str {
        PROVIDER_ID
    }

    fn recognize(&self, image_bytes: &[u8]) -> anyhow::Result<OcrResult> {
        // 分平台实现：桌面平台走真 ONNX 推理，其余平台返回明确错误。
        self.recognize_impl(image_bytes)
    }

    fn requires_download(&self) -> bool {
        true
    }

    fn download_status(&self) -> Option<Vec<DownloadStatus>> {
        Some(all_download_status())
    }
}

impl RapidOcrEngine {
    /// 桌面平台：RapidOCR ONNX 推理入口。
    /// （固有方法，不能写在 trait impl 里；用 cfg 门控的 fn 而非表达式上的 cfg，
    ///  后者在稳定版 rustc 是实验特性。）
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn recognize_impl(&self, image_bytes: &[u8]) -> anyhow::Result<OcrResult> {
        self.recognize_onnx(image_bytes)
    }

    /// 其余平台（Linux/移动端）：rapidocr-core 未编译，直接报平台不支持。
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn recognize_impl(&self, _image_bytes: &[u8]) -> anyhow::Result<OcrResult> {
        Err(anyhow::anyhow!(
            "RapidOCR ONNX 推理暂不支持当前平台（仅 Windows / macOS）"
        ))
    }
}

/// 取消所有进行中的模型下载（直接操作全局状态表，不依赖具体引擎实例或当前偏好）。
/// 下载循环每写一个 chunk 检查一次状态，发现取消会删除半成品文件并退出。
pub fn cancel_all_downloads() {
    let mut map = DOWNLOADS.lock().unwrap();
    for status in map.values_mut() {
        if status.state == DownloadState::InProgress {
            status.state = DownloadState::Cancelled;
        }
    }
}

/// 返回全局下载状态表快照（命令层轮询用）。
pub fn all_download_status() -> Vec<DownloadStatus> {
    DOWNLOADS.lock().unwrap().values().cloned().collect()
}

async fn download_one(model_id: &str, url: &str, target_dir: &Path) -> DownloadStatus {
    let target_dir = target_dir.to_path_buf();
    let filename = url.rsplit('/').next().unwrap_or(model_id);
    let target = target_dir.join(filename);

    if let Err(e) = std::fs::create_dir_all(&target_dir) {
        return DownloadStatus {
            provider_id: PROVIDER_ID.into(),
            model_id: model_id.into(),
            state: DownloadState::Failed,
            error: Some(format!("create dir: {e}")),
            ..Default::default()
        };
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build();
    let client = match client {
        Ok(c) => c,
        Err(e) => {
            return DownloadStatus {
                provider_id: PROVIDER_ID.into(),
                model_id: model_id.into(),
                state: DownloadState::Failed,
                error: Some(format!("build client: {e}")),
                ..Default::default()
            }
        }
    };

    let head = client.head(url).send().await;
    let total = match &head {
        Ok(resp) => resp
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok()),
        Err(_) => None,
    };

    let response = match client.get(url).send().await {
        Ok(r) => r,
        Err(e) => {
            return DownloadStatus {
                provider_id: PROVIDER_ID.into(),
                model_id: model_id.into(),
                state: DownloadState::Failed,
                error: Some(format!("request: {e}")),
                bytes_total: total,
                ..Default::default()
            }
        }
    };

    let mut stream = response.bytes_stream();
    let mut downloaded: u64 = 0;
    let mut file = match tokio::fs::File::create(&target).await {
        Ok(f) => f,
        Err(e) => {
            return DownloadStatus {
                provider_id: PROVIDER_ID.into(),
                model_id: model_id.into(),
                state: DownloadState::Failed,
                error: Some(format!("create file: {e}")),
                bytes_total: total,
                ..Default::default()
            }
        }
    };

    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                return DownloadStatus {
                    provider_id: PROVIDER_ID.into(),
                    model_id: model_id.into(),
                    state: DownloadState::Failed,
                    error: Some(format!("download stream: {e}")),
                    bytes_total: total,
                    bytes_downloaded: Some(downloaded),
                };
            }
        };
        downloaded += chunk.len() as u64;
        if let Err(e) = file.write_all(&chunk).await {
            return DownloadStatus {
                provider_id: PROVIDER_ID.into(),
                model_id: model_id.into(),
                state: DownloadState::Failed,
                error: Some(format!("write file: {e}")),
                bytes_total: total,
                bytes_downloaded: Some(downloaded),
            };
        }

        // 更新进度。
        {
            let mut map = DOWNLOADS.lock().unwrap();
            if let Some(s) = map.get_mut(model_id) {
                s.bytes_downloaded = Some(downloaded);
                s.bytes_total = total;
                if s.state == DownloadState::Cancelled {
                    let _ = std::fs::remove_file(&target);
                    return DownloadStatus {
                        provider_id: PROVIDER_ID.into(),
                        model_id: model_id.into(),
                        state: DownloadState::Cancelled,
                        error: None,
                        bytes_total: total,
                        bytes_downloaded: Some(downloaded),
                    };
                }
            }
        }
    }

    DownloadStatus {
        provider_id: PROVIDER_ID.into(),
        model_id: model_id.into(),
        state: DownloadState::Completed,
        error: None,
        bytes_total: total,
        bytes_downloaded: Some(downloaded),
    }
}

/// 计算模型下载 URL。
///
/// - `mirror` 为空：直接使用默认 GitHub 地址；
/// - `mirror` 非空：拼接 `{mirror}/{filename}`，自动去掉 mirror 尾部多余斜杠。
///
/// `mirror` 的语义与设置页「下载镜像」输入框一致：应填到 release 目录前缀
/// （例如 `https://mirror.example.com/rapidocr/v2.0.0`），应用会自动追加 `/模型文件名.onnx`。
pub fn model_download_url(mirror: &str, filename: &str, default_url: &str) -> String {
    if mirror.is_empty() {
        default_url.to_string()
    } else {
        format!("{}/{filename}", mirror.trim_end_matches('/'))
    }
}

use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;

// ===== ONNX 推理实现（仅桌面平台编译；rapidocr-core / ort 依赖见 Cargo.toml） =====

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl RapidOcrEngine {
    /// 真正的 ONNX 推理入口：模型存在性 → 解码图片 → 初始化 ort →
    /// 构建/复用 `RapidOcr` 管线 → 跑 det/cls/rec → 转成内部 `OcrResult`。
    fn recognize_onnx(&self, image_bytes: &[u8]) -> anyhow::Result<OcrResult> {
        if !self.all_model_files_exist() {
            return Err(anyhow::anyhow!(
                "RapidOCR 模型尚未下载完成，请在「设置 → 服务 → OCR」中下载模型"
            ));
        }

        let image = image::load_from_memory(image_bytes)
            .map_err(|e| anyhow::anyhow!("解码 OCR 输入图片失败: {e}"))?;
        let preprocessed = preprocess_for_ocr(image);
        let rgb = preprocessed.to_rgb8();
        if rgb.width() == 0 || rgb.height() == 0 {
            return Err(anyhow::anyhow!("OCR 输入图片为空"));
        }

        // ort 环境只需初始化一次（内部 OnceLock，幂等）。必须在首次创建
        // Session 之前调用，否则 ort 会尝试从可执行文件旁加载并 panic。
        self.init_ort()?;

        let mut pipeline = self.pipeline.lock().map_err(|_| {
            anyhow::anyhow!("rapidocr pipeline 锁被污染（poisoned）")
        })?;
        if pipeline.is_none() {
            let cfg = Self::build_config(&self.models_dir)?;
            let ocr = RapidOcr::new(cfg)
                .map_err(|e| anyhow::anyhow!("初始化 RapidOCR 管线失败: {e:#}"))?;
            log::info!("[ocr] RapidOCR 管线已就绪（det/cls/rec session 加载完成）");
            *pipeline = Some(ocr);
        }

        let start = std::time::Instant::now();
        let output = pipeline
            .as_mut()
            .expect("pipeline just initialized above")
            .run_image(&rgb)
            .map_err(|e| anyhow::anyhow!("RapidOCR 推理失败: {e:#}"))?;
        log::debug!(
            "[ocr] rapidocr recognized {} lines in {:?}",
            output.lines.len(),
            start.elapsed()
        );

        Ok(OcrResult {
            lines: output
                .lines
                .into_iter()
                .map(|line| super::engine::OcrLine {
                    text: line.text,
                    confidence: Some(line.score),
                })
                .collect(),
        })
    }
}

/// 对输入图片做 OCR 前预处理：灰度化 + 对比度拉伸，提升小字、彩色气泡、
/// 低对比度界面文字的识别率。
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn preprocess_for_ocr(image: image::DynamicImage) -> image::DynamicImage {
    // 1) 灰度化：去掉颜色干扰，让 det/rec 更关注亮度结构。
    let gray = image.to_luma8();

    // 2) 线性对比度拉伸：把当前最小/最大亮度映射到 0/255，增强浅灰文字与背景的区分。
    let stretched = contrast_stretch(&gray);

    // 3) 转回 RGB8，因为 rapidocr-core 的 run_image 需要 3 通道图像。
    let rgb = image::ImageBuffer::from_fn(stretched.width(), stretched.height(), |x, y| {
        let v = stretched.get_pixel(x, y)[0];
        image::Rgb([v, v, v])
    });
    image::DynamicImage::ImageRgb8(rgb)
}

/// 对 8bit 灰度图做 min-max 线性对比度拉伸。
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn contrast_stretch(gray: &image::GrayImage) -> image::GrayImage {
    let (min, max) = gray.pixels().fold((255u8, 0u8), |(min, max), p| {
        let v = p[0];
        (min.min(v), max.max(v))
    });
    if min == max || (max - min) == 255 {
        return gray.clone();
    }
    let range = (max - min) as f32;
    let min = min as f32;
    image::ImageBuffer::from_fn(gray.width(), gray.height(), |x, y| {
        let v = gray.get_pixel(x, y)[0] as f32;
        let stretched = ((v - min) / range * 255.0).round() as u8;
        image::Luma([stretched])
    })
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl RapidOcrEngine {
    /// 构建 PP-OCRv6 small 管线配置，模型路径指向本引擎的模型目录。
    ///
    /// `RapidOcrConfig::ppocr_v6_small` 注册表里 rec 模型文件名是
    /// `PP-OCRv6_rec_small.onnx`，而本应用下载的是 GitHub release 的
    /// `ch_PP-OCRv6_rec_small.onnx`（同一模型，仅文件名不同），这里手动对齐；
    /// 字典 `ppocrv6_dict.txt` 已加入下载清单。
    fn build_config(models_dir: &Path) -> anyhow::Result<RapidOcrConfig> {
        let cfg = RapidOcrConfig::ppocr_v6_small(models_dir);
        cfg.validate()
            .map_err(|e| anyhow::anyhow!("RapidOCR 配置无效: {e:#}"))?;
        Ok(cfg)
    }

    /// 初始化 ort 的 ONNX Runtime 动态库（`load-dynamic` 模式）。
    ///
    /// 查找顺序：`ORT_DYLIB_PATH` 环境变量 → 模型目录 → 可执行文件旁 →
    /// 开发环境 `FOUNDRY_NATIVE_OVERRIDE_DIR`（foundry-local-sdk 的 DLL，
    /// 仅 dev 便利，其 1.26.0 版本满足 ort 2.0.0-rc.12 的最低 API 版本）。
    /// 生产打包后 onnxruntime.dll 随 foundry-dlls 一起放在可执行文件旁，
    /// 命中第 3 条。
    fn init_ort(&self) -> anyhow::Result<()> {
        let Some(path) = Self::find_onnxruntime_dylib(&self.models_dir) else {
            return Err(anyhow::anyhow!(
                "未找到 ONNX Runtime 动态库（onnxruntime.dll / libonnxruntime.dylib）。\
                 请将其放到模型目录或可执行文件旁，或设置 ORT_DYLIB_PATH 环境变量"
            ));
        };
        ort::init_from(&path)
            .map_err(|e| {
                anyhow::anyhow!("加载 ONNX Runtime 动态库失败 {}: {e}", path.display())
            })?
            .commit();
        log::info!("[ocr] ort 已加载 ONNX Runtime 动态库: {}", path.display());
        Ok(())
    }

    /// 在候选位置查找 ONNX Runtime 动态库，返回第一个存在的路径。
    fn find_onnxruntime_dylib(models_dir: &Path) -> Option<PathBuf> {
        #[cfg(target_os = "windows")]
        let dylib_name = "onnxruntime.dll";
        #[cfg(target_os = "macos")]
        let dylib_name = "libonnxruntime.dylib";
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        let dylib_name = "libonnxruntime.so";

        let mut candidates: Vec<PathBuf> = Vec::new();
        // 1. ort 官方约定的环境变量（路径可相对可执行文件）。
        if let Ok(p) = std::env::var("ORT_DYLIB_PATH") {
            if !p.is_empty() {
                candidates.push(PathBuf::from(p));
            }
        }
        // 2. 模型目录（可与模型一起分发/手动放置）。
        candidates.push(models_dir.join(dylib_name));
        // 3. 可执行文件旁（打包后 onnxruntime.dll 随 foundry-dlls 在此）。
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                candidates.push(dir.join(dylib_name));
            }
        }
        // 4. 开发环境：foundry-local-sdk 的 DLL 目录（FOUNDRY_NATIVE_OVERRIDE_DIR）。
        if let Ok(dir) = std::env::var("FOUNDRY_NATIVE_OVERRIDE_DIR") {
            if !dir.is_empty() {
                candidates.push(PathBuf::from(dir).join(dylib_name));
            }
        }

        candidates.into_iter().find(|p| p.is_file())
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "windows")))]
mod tests {
    use super::*;

    /// 配置路径必须指向 start_download 实际下载的文件名。
    #[test]
    fn build_config_points_to_downloaded_model_filenames() {
        let cfg = RapidOcrEngine::build_config(Path::new("fake-models")).unwrap();

        let det = cfg.det.as_ref().unwrap();
        assert_eq!(
            det.model_path,
            PathBuf::from("fake-models/PP-OCRv6_det_small.onnx")
        );
        let cls = cfg.cls.as_ref().unwrap();
        assert_eq!(
            cls.model_path,
            PathBuf::from("fake-models/ch_ppocr_mobile_v2.0_cls_mobile.onnx")
        );
        let rec = cfg.rec.as_ref().unwrap();
        assert_eq!(
            rec.model_path,
            PathBuf::from("fake-models/PP-OCRv6_rec_small.onnx")
        );
        assert_eq!(
            rec.dict_path,
            PathBuf::from("fake-models/ppocrv6_dict.txt")
        );
        assert!(cfg.pipeline.use_det);
        assert!(cfg.pipeline.use_cls);
        assert!(cfg.pipeline.use_rec);
    }

    /// 下载清单必须包含 rec 阶段必需的字典文件。
    #[test]
    fn model_set_includes_dictionary_asset() {
        assert_eq!(RAPIDOCR_MODEL_SET.len(), 4);
        assert!(RAPIDOCR_MODEL_SET
            .iter()
            .any(|(filename, _)| *filename == "ppocrv6_dict.txt"));
    }

    fn sample() -> (&'static str, &'static str) {
        let (filename, url) = RAPIDOCR_MODEL_SET[0];
        (filename, url)
    }

    /// 设置页「下载镜像」输入框填 release 目录前缀时的拼接结果。
    #[test]
    fn mirror_prefix_joins_filename() {
        let (filename, url) = sample();
        let mirror = "https://mirror.example.com/rapidocr/v2.0.0";
        let joined = model_download_url(mirror, filename, url);
        assert_eq!(
            joined,
            format!("https://mirror.example.com/rapidocr/v2.0.0/{filename}")
        );
        // download_one 从 URL 末段取文件名，拼接结果必须保持文件名在末尾。
        assert_eq!(joined.rsplit('/').next().unwrap(), filename);
    }

    /// 镜像带尾部斜杠时自动去除，避免双斜杠。
    #[test]
    fn mirror_trailing_slash_is_trimmed() {
        let (filename, url) = sample();
        assert_eq!(
            model_download_url("https://mirror.example.com/rapidocr/v2.0.0/", filename, url),
            format!("https://mirror.example.com/rapidocr/v2.0.0/{filename}")
        );
    }

    /// 镜像为空时回退到默认 GitHub 地址，全部模型都能拿到完整 URL。
    #[test]
    fn empty_mirror_uses_default_urls() {
        for (filename, url) in RAPIDOCR_MODEL_SET {
            let joined = model_download_url("", filename, url);
            assert_eq!(joined, *url);
            assert!(joined.ends_with(filename));
        }
    }
}
