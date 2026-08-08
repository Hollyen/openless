//! Tauri commands for OCR / screen-context provider management.

use crate::ocr::{self, DownloadStatus, OcrProvider, rapidocr};
use crate::types::UserPreferences;

use super::CoordinatorState;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrSettings {
    pub screen_context_enabled: bool,
    pub active_provider_id: String,
    pub providers: Vec<OcrProvider>,
    pub models_base_dir: String,
    pub models_root_dir: String,
    pub download_mirror: String,
    pub requires_download: bool,
}

#[tauri::command]
pub fn get_ocr_settings(coord: CoordinatorState<'_>) -> OcrSettings {
    let prefs = coord.prefs().get();
    let providers = ocr::available_providers();
    let requires_download = active_provider_requires_download(&prefs, &providers);
    OcrSettings {
        screen_context_enabled: prefs.screen_context_enabled,
        active_provider_id: prefs.active_ocr_provider.clone(),
        providers,
        models_base_dir: prefs.ocr_models_base_dir.clone(),
        models_root_dir: ocr_models_root(&prefs),
        download_mirror: prefs.ocr_download_mirror.clone(),
        requires_download,
    }
}

#[tauri::command]
pub fn set_ocr_settings(
    coord: CoordinatorState<'_>,
    screen_context_enabled: bool,
    active_provider_id: String,
    models_base_dir: String,
    download_mirror: String,
) -> Result<(), String> {
    let mut prefs = coord.prefs().get();
    prefs.screen_context_enabled = screen_context_enabled;
    prefs.active_ocr_provider = active_provider_id;
    prefs.ocr_models_base_dir = models_base_dir.trim().to_string();
    prefs.ocr_download_mirror = download_mirror.trim().to_string();
    coord.prefs().set(prefs).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_ocr_providers() -> Vec<OcrProvider> {
    ocr::available_providers()
}

#[tauri::command]
pub fn start_ocr_model_download(
    coord: CoordinatorState<'_>,
    mirror: Option<String>,
) -> Result<(), String> {
    let prefs = coord.prefs().get();
    let mirror = mirror.unwrap_or_else(|| prefs.ocr_download_mirror.clone());
    rapidocr::RapidOcrEngine::new(&prefs).start_download(&mirror);
    Ok(())
}

#[tauri::command]
pub fn cancel_ocr_model_download() -> Result<(), String> {
    // 直接操作全局下载状态表，不依赖具体引擎实例或当前偏好。
    rapidocr::cancel_all_downloads();
    Ok(())
}

#[tauri::command]
pub fn get_ocr_download_status() -> Vec<DownloadStatus> {
    rapidocr::all_download_status()
}

#[tauri::command]
pub fn delete_ocr_models(coord: CoordinatorState<'_>) -> Result<(), String> {
    let prefs = coord.prefs().get();
    rapidocr::RapidOcrEngine::new(&prefs)
        .delete_models()
        .map_err(|e| format!("{e:#}"))
}

fn ocr_models_root(prefs: &UserPreferences) -> String {
    // 与 RapidOcrEngine::new 使用同一目录计算，保证设置页展示与实际落盘路径一致。
    rapidocr::RapidOcrEngine::models_dir_for(prefs)
        .display()
        .to_string()
}

fn active_provider_requires_download(prefs: &UserPreferences, providers: &[OcrProvider]) -> bool {
    providers
        .iter()
        .find(|p| p.id == prefs.active_ocr_provider)
        .map(|p| p.requires_download)
        .unwrap_or(false)
}

#[tauri::command]
pub async fn test_ocr_recognition(coord: CoordinatorState<'_>) -> Result<Option<String>, String> {
    let prefs = coord.prefs().get();
    if !prefs.screen_context_enabled {
        return Ok(None);
    }
    ocr::recognize_screen_text(&prefs)
        .await
        .map_err(|e| format!("{e:#}"))
}
