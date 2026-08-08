import type { OcrProvider, DownloadStatus } from "../types"
import { invokeOrMock } from "./shared"
import { mockSettings } from "./mock-data"

/**
 * 后端错误标记：macOS 屏幕录制（TCC）权限未开启。
 * `test_ocr_recognition` 的错误消息以该前缀开头时，设置页应展示系统设置跳转引导。
 */
export const OCR_ERROR_SCREEN_RECORDING_DENIED = "screenRecordingDenied"

/**
 * 后端错误标记：Windows WinRT OCR 缺少中文识别语言包。
 * `test_ocr_recognition` 的错误消息以该前缀开头时，设置页应展示语言包安装引导。
 */
export const OCR_ERROR_WINRT_LANGUAGE_MISSING = "winrtChineseOcrLanguageMissing"

export interface OcrSettings {
    screenContextEnabled: boolean
    activeProviderId: string
    providers: OcrProvider[]
    modelsBaseDir: string
    modelsRootDir: string
    downloadMirror: string
    requiresDownload: boolean
}

export function getOcrSettings(): Promise<OcrSettings> {
    return invokeOrMock(
        "get_ocr_settings",
        undefined,
        () => ({
            screenContextEnabled: mockSettings.screenContextEnabled,
            activeProviderId: mockSettings.activeOcrProvider,
            providers: [
                {
                    id: "disabled",
                    name: "关闭",
                    description: "不注入屏幕上下文",
                    local: true,
                    requiresDownload: false,
                    supportedPlatforms: ["windows", "macos", "linux"],
                },
                {
                    id: "rapidocr",
                    name: "RapidOCR (PP-OCRv6)",
                    description: "本地 ONNX 模型，中文识别质量好，需下载约 30 MB 模型",
                    local: true,
                    requiresDownload: true,
                    supportedPlatforms: ["windows", "macos", "linux"],
                },
            ],
            modelsBaseDir: mockSettings.ocrModelsBaseDir,
            modelsRootDir: "",
            downloadMirror: mockSettings.ocrDownloadMirror,
            requiresDownload: false,
        }),
    )
}

export function setOcrSettings(settings: {
    screenContextEnabled: boolean
    activeProviderId: string
    modelsBaseDir: string
    downloadMirror: string
}): Promise<void> {
    return invokeOrMock(
        "set_ocr_settings",
        {
            screenContextEnabled: settings.screenContextEnabled,
            activeProviderId: settings.activeProviderId,
            modelsBaseDir: settings.modelsBaseDir,
            downloadMirror: settings.downloadMirror,
        },
        () => {
            mockSettings.screenContextEnabled = settings.screenContextEnabled
            mockSettings.activeOcrProvider = settings.activeProviderId
            mockSettings.ocrModelsBaseDir = settings.modelsBaseDir
            mockSettings.ocrDownloadMirror = settings.downloadMirror
        },
    )
}

export function getOcrProviders(): Promise<OcrProvider[]> {
    return invokeOrMock("get_ocr_providers", undefined, () => [])
}

export function startOcrModelDownload(mirror?: string): Promise<void> {
    return invokeOrMock("start_ocr_model_download", { mirror }, () => undefined)
}

export function cancelOcrModelDownload(): Promise<void> {
    return invokeOrMock("cancel_ocr_model_download", undefined, () => undefined)
}

export function getOcrDownloadStatus(): Promise<DownloadStatus[]> {
    return invokeOrMock("get_ocr_download_status", undefined, () => [])
}

export function deleteOcrModels(): Promise<void> {
    return invokeOrMock("delete_ocr_models", undefined, () => undefined)
}

export function testOcrRecognition(): Promise<string | null> {
    return invokeOrMock("test_ocr_recognition", undefined, () => null)
}
