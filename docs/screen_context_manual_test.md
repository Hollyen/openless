# 屏幕上下文（截图 + OCR）手动测试清单

> 状态：Phase 5 集成审查产出（只读/小修阶段）
> 范围：桌面端（Windows / macOS）；移动端明确不做（见设计文档 §1.2）
> 关联代码：`app/src-tauri/src/{ocr,commands/ocr.rs,coordinator/screen_context.rs,polish/prompt_compose.rs}`、`app/src/lib/ipc/ocr.ts`、`app/src/pages/settings/OcrSection.tsx`

前置条件：

- Rust 工具链就绪后先执行 `cargo update` 锁定 `xcap = "0.9"`（当前 Cargo.lock 未包含 xcap，离线构建会失败），再 `cargo check` / `cargo test`。
- 桌面 `npm run tauri dev` 启动应用。
- 测试期间确认系统级权限：Windows 无需额外权限；macOS 首次触发截图会弹「屏幕录制」TCC 授权，需在「系统设置 → 隐私与安全性 → 屏幕录制」中允许 OpenLess（Info.plist 已含 `NSScreenCaptureUsageDescription`）。

---

## A. 设置页基本交互（两个平台通用）

| # | 操作 | 预期结果 |
|---|------|----------|
| A1 | 打开「设置 → 服务 → OCR」 | 开关「屏幕上下文」默认关闭；引擎下拉显示「关闭 / RapidOCR (PP-OCRv6)」（Windows 额外有「Windows 原生 OCR」） |
| A2 | 切换引擎为 RapidOCR | 显示模型目录路径（应指向 `<prefs.ocr_models_base_dir 或默认模型根>/rapidocr`），出现「需要下载模型」提示与「开始下载」按钮 |
| A3 | 切换引擎为 disabled（关闭） | 下载/删除/测试相关按钮隐藏或禁用 |
| A4 | 修改「模型目录」为自定义路径后保存 | 重启应用后设置仍生效（`set_ocr_settings` 持久化到偏好） |
| A5 | 修改「下载镜像」为自定义 URL 前缀后保存 | 设置持久化；下载时 URL 拼接为 `<mirror>/<文件名>` |
| A6 | 修改设置后重启应用 | 引擎选择、目录、镜像、开关均恢复 |

## B. 模型下载 / 取消 / 删除（RapidOCR）

| # | 操作 | 预期结果 |
|---|------|----------|
| B1 | 点击「开始下载」 | 3 个模型（det/rec/cls .onnx）逐个下载，进度列表实时刷新（`get_ocr_download_status` 轮询） |
| B2 | 下载中途点击「取消」 | 状态变 `cancelled`，半成品文件被清理（`cancel_ocr_model_download` 全局取消） |
| B3 | 取消后重新点击「开始下载」 | 能正常重新发起下载 |
| B4 | 断网状态下点击「开始下载」 | 状态变 `failed` 且 error 字段有可读中文错误；不崩溃 |
| B5 | 下载完成后点击「删除模型」 | 模型目录被整目录删除，状态表清空；再次显示「需要下载」 |
| B6 | 自定义模型目录场景下下载 | 文件落在 `<自定义目录>/rapidocr/` 下（不是嵌套 `<...>/models/rapidocr`）；设置页展示路径与实际一致 |
| B7 | 未下载完成时在其它界面触发润色 | 不崩溃；RapidOCR 引擎 `ensure_model_downloaded` 报「模型尚未下载完成」错误，润色走无屏幕上下文降级路径 |

## C. 识别测试（设置页「测试识别」按钮）

| # | 操作 | 预期结果 |
|---|------|----------|
| C1 | 引擎 disabled 时点「测试识别」 | 返回空（不做截图/OCR，直接短路） |
| C2 | Windows + WinRT 引擎、系统已装中文 OCR 语言包，屏幕上有清晰中文文本时点「测试识别」 | 返回识别出的文本（`test_ocr_recognition`） |
| C3 | Windows + WinRT、未安装中文语言包 | 回退 `TryCreateFromUserProfileLanguages`；可能返回英文/空结果，控制台有 warn；不崩溃 |
| C4 | RapidOCR 引擎、模型齐全时点「测试识别」 | **已知行为**：当前返回空（recognize() 是 stub，ONNX 推理待后续 PR）；日志有 `RapidOCR recognize() is a stub` warn |
| C5 | RapidOCR 引擎、模型缺失时点「测试识别」 | 返回清晰中文错误提示（引导去下载模型） |
| C6 | macOS 未授权「屏幕录制」时点「测试识别」 | 截图失败 → 返回错误或空，控制台记录原因；不崩溃（后续版本应加应用内权限引导） |
| C7 | macOS 授权屏幕录制后点「测试识别」（RapidOCR 模型齐全前先确认 B 完成） | 截图成功；由于 RapidOCR stub 返回空文本，预期空结果 + warn（与 C4 一致） |

## D. 屏幕上下文注入润色链路

| # | 操作 | 预期结果 |
|---|------|----------|
| D1 | 开启屏幕上下文 + 任意可用 OCR 引擎，屏幕上放一段中文文本，进行一次听写润色 | 日志出现 `[screen_context] captured N chars`；润色结果正常返回 |
| D2 | 内置风格包 prompt 含 `{{SCREEN_CONTEXT}}` 占位符 | 屏幕上下文块替换到占位符位置（`assemble_polish_prompts`） |
| D3 | 自定义 prompt 不含占位符 | 屏幕上下文块自动追加到 prompt 末尾 |
| D4 | 屏幕上下文关闭时润色 | 日志不出现 capture 行；prompt 无屏幕上下文块（`includes_screen_context_block=false`） |
| D5 | OCR 超时（慢速场景，可临时把超时改小验证） | 5 秒超时降级，润色继续走无上下文路径；不阻塞听写 |
| D6 | 听写链路在 OCR 进行中开始新一轮听写 | 听写不因 OCR 阻塞（OCR 在 `spawn_blocking` + 超时保护内） |
| D7 | 选中文本润色（selection polish） | 不携带屏幕上下文（`selection_polish` 传 `None`），行为与 v1 前一致 |
| D8 | 屏幕上下文超长（> 截断阈值） | 截断为 `...[truncated]` 后缀，无 UTF-8 截断乱码（`safe_str_slice`） |
| D9 | 屏幕文本含 `<`/`>` 等 XML 特殊字符 | 被 XML 转义/信封包裹，不破坏 system prompt 结构（注入防护） |

## E. 平台专项

| # | 平台 | 操作 | 预期结果 |
|---|------|------|----------|
| E1 | Windows | 用 WinRT 引擎完整跑一遍 D 链路 | `Windows.Media.Ocr` 识别屏幕文本，中文正常 |
| E2 | Windows | 验证无 %TEMP% 临时文件产生 | OCR 全程内存解码（`InMemoryRandomAccessStream`），不落盘 |
| E3 | macOS | 首次润色触发截图 | 系统弹出屏幕录制授权；拒绝后润色降级不崩溃 |
| E4 | macOS | 授权后在无头/锁屏状态触发润色 | 截图失败降级 `None`，日志有 warn |
| E5 | Linux | 开启屏幕上下文并润色 | 不崩溃；`capture_primary_screen` 走 cfg stub 返回 `None` + warn「not yet implemented on this platform」 |
| E6 | 移动端（Android/iOS） | 调用 `ocr.ts` 任意命令 | **已知限制**：命令未注册在移动端 invoke_handler，报 command not found；设计上移动端不做屏幕上下文（仅桌面） |

## F. 安全与隐私

| # | 操作 | 预期结果 |
|---|------|----------|
| F1 | 屏幕上有敏感内容（密码、聊天）时润色 | 截图仅本地处理，不出进程（无网络上传路径）；Prompt 中屏幕上下文仅作为参考 XML 块 |
| F2 | 关闭屏幕上下文后检查后台 | 不再有任何截图/OCR 调用（`active_engine` 短路 disabled） |
| F3 | 删除模型后再检查磁盘 | 模型目录已删除，无残留 |

## G. 回归（确保未破坏既有功能）

| # | 操作 | 预期结果 |
|---|------|----------|
| G1 | 全部既有听写/润色/翻译流程（关闭屏幕上下文默认态） | 与 v1 前行为完全一致 |
| G2 | `cargo test`（prompt_compose 屏幕上下文单元测试） | 3 个新增测试通过（截断+信封闭合、XML 注入中和、assemble/preview 联动） |
| G3 | `tsc --noEmit` + `vitest` | 无 TS 错误；stylePrefs/mock-data/android-ipc-import-boundary 测试通过 |
| G4 | Windows 上设置页「测试识别」在引擎切换各状态下重复点击 | 无 panic、无 UI 卡死 |
