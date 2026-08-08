//! Windows 原生 WinRT OCR 引擎 (`Windows.Media.Ocr`)。
//!
//! 依赖：
//! - `xcap` 截图 → `image` PNG 编码（在 `ocr/mod.rs`）
//! - `windows` crate 的 WinRT 命名空间：`Media_Ocr`, `Graphics_Imaging`, `Storage_Streams`, `Globalization`
//!
//! 流程：
//! 1. 线程内 `CoInitializeEx` 初始化 COM/WinRT（重复初始化返回 S_FALSE，忽略返回值安全；
//!    本模块在 `tokio::task::spawn_blocking` 线程上运行，无死锁风险）
//! 2. PNG 字节写入 `InMemoryRandomAccessStream`（不落临时文件——非打包应用经
//!    `StorageFile::GetFileFromPathAsync` 访问 AppData 下路径会触发
//!    `UNABLE_TO_MASK_PATH` 0x800700A1）
//! 3. `BitmapDecoder::CreateAsync` 解码为 `SoftwareBitmap`
//! 4. `OcrEngine::RecognizeAsync` 识别并返回行文本

use windows::core::HSTRING;
use windows::Graphics::Imaging::BitmapDecoder;
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

use super::engine::{OcrEngine as OcrEngineTrait, OcrLine, OcrProvider, OcrResult};

const PROVIDER_ID: &str = "winrt";

pub struct WinRtOcrEngine;

impl WinRtOcrEngine {
    pub fn new() -> Self {
        Self
    }

    pub fn provider_list() -> Vec<OcrProvider> {
        vec![OcrProvider {
            id: PROVIDER_ID.into(),
            name: "Windows 原生 OCR".into(),
            description: "调用 Windows 10/11 内置的 Windows.Media.Ocr，无需下载模型，依赖系统已安装的 OCR 语言包".into(),
            local: true,
            requires_download: false,
            supported_platforms: vec!["windows".into()],
        }]
    }
}

impl Default for WinRtOcrEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl OcrEngineTrait for WinRtOcrEngine {
    fn provider_id(&self) -> &str {
        PROVIDER_ID
    }

    fn recognize(&self, image_bytes: &[u8]) -> anyhow::Result<OcrResult> {
        // 每个调用线程都初始化 COM/WinRT；重复初始化返回 S_FALSE，安全。
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED).ok();
        }

        // PNG 字节 → 内存流 → BitmapDecoder → SoftwareBitmap。
        let stream = png_bytes_to_stream(image_bytes)?;
        let decoder = BitmapDecoder::CreateAsync(&stream)?.get()?;
        let bitmap = decoder.GetSoftwareBitmapAsync()?.get()?;

        // 语言回退：优先 zh-Hans-CN（中文系统 / 已装中文 OCR 语言包）；
        // 失败则回退到用户配置文件语言。
        // 两者都失败说明系统缺少 OCR 语言包（尤其 zh-Hans-CN 对应的中文 OCR 语言包），
        // 返回带 `winrtChineseOcrLanguageMissing` 标记的错误，供设置页展示安装引导。
        let engine = match create_engine_for_language("zh-Hans-CN") {
            Ok(engine) => engine,
            Err(zh_err) => match OcrEngine::TryCreateFromUserProfileLanguages() {
                Ok(engine) => {
                    log::warn!(
                        "[ocr] failed to create zh-Hans-CN engine: {zh_err:#}, falling back to user profile languages"
                    );
                    engine
                }
                Err(profile_err) => {
                    return Err(anyhow::anyhow!(
                        "winrtChineseOcrLanguageMissing: WinRT OCR cannot create a recognition engine for zh-Hans-CN ({zh_err:#}) or user profile languages ({profile_err:#}); install the Chinese OCR language pack"
                    ));
                }
            },
        };

        let result = engine.RecognizeAsync(&bitmap)?.get()?;
        let lines = result.Lines()?;
        let mut out = Vec::new();
        for line in lines {
            let text = line.Text()?;
            out.push(OcrLine {
                text: text.to_string_lossy(),
                confidence: None,
            });
        }

        Ok(OcrResult { lines: out })
    }

    fn requires_download(&self) -> bool {
        false
    }
}

/// 优先创建指定语言标签的 OCR 引擎（如 `zh-Hans-CN`）。
fn create_engine_for_language(tag: &str) -> windows::core::Result<OcrEngine> {
    let lang = windows::Globalization::Language::CreateLanguage(&HSTRING::from(tag))?;
    OcrEngine::TryCreateFromLanguage(&lang)
}

/// 把 PNG 字节写入内存流并回卷到起始位置，供 `BitmapDecoder` 解码。
fn png_bytes_to_stream(image_bytes: &[u8]) -> windows::core::Result<InMemoryRandomAccessStream> {
    let stream = InMemoryRandomAccessStream::new()?;
    let writer = DataWriter::CreateDataWriter(&stream)?;
    writer.WriteBytes(image_bytes)?;
    writer.StoreAsync()?.get()?;
    writer.FlushAsync()?.get()?;
    stream.Seek(0)?;
    Ok(stream)
}
