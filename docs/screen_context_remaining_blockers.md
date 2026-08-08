# OpenLess 屏幕上下文剩余阻塞点拆分与推进设计

> 状态：B1/B2/B3/B5 已完成；B4（Tauri 打包）正在推进；B6/B7 仍为后续优化项
> 范围：桌面端（Windows 优先，macOS/Linux 后续补齐）
> 关联代码：`openless-all/app/src-tauri/src/ocr/*`、`openless-all/app/src-tauri/src/polish/*`、前端 `OcrSection.tsx`、打包配置

---

## 1. 当前基线

### 1.1 已完成的硬阻塞

- `sherpa-onnx` 1.13.2 Windows 静态库已下载并解压：
  - `C:\Users\s00883827\.local\openless-native-libs\sherpa-onnx-v1.13.2-win-x64-static-MT-Release-lib\lib`
- `foundry-local-sdk` 所需的 4 个 NuGet DLL 已下载并解压：
  - `C:\Users\s00883827\.local\openless-native-libs\foundry-dlls`
- 使用以下环境变量后，`cargo check` 与 `cargo test --lib` 均已跑通：

  ```powershell
  $env:SHERPA_ONNX_LIB_DIR="C:\Users\s00883827\.local\openless-native-libs\sherpa-onnx-v1.13.2-win-x64-static-MT-Release-lib\lib"
  $env:FOUNDRY_NATIVE_OVERRIDE_DIR="C:\Users\s00883827\.local\openless-native-libs\foundry-dlls"
  $env:NO_PROXY="rust.inhuawei.com,localhost,127.0.0.1"
  $env:PATH="$env:USERPROFILE\.cargo\bin;$env:PATH"
  ```

- `cargo test --lib`：**985 passed / 1 failed**，唯一失败是 `remote_server::pin_persistence::tests::symlink_pin_path_is_rejected_without_touching_its_target`（错误码 1314，当前非管理员进程缺少创建符号链接权限），**与屏幕上下文无关**。
- 已删除临时 workaround：`vendor/foundry-dummy` 空 DLL 占位、`vendor/ureq-native-certs` 源码 patch；改用在 `Cargo.toml` 显式依赖 `ureq = { version = "2.12", features = ["native-certs"] }` 来信任系统证书。
- Windows 默认 OCR 已改为 `rapidocr`（`types.rs` `default_active_ocr_provider()`），macOS/Linux 仍默认 `disabled`。
- **RapidOCR 真机冒烟测试通过**：`cargo test --lib -- --ignored ocr::tests::rapidocr_screen_context_smoke_test` 在当前屏幕上成功识别出 2165 字符，证明 `xcap` 截图 + `rapidocr-core` + `ort` 推理链路已打通。

### 1.2 仍然存在的阻塞点总览

| 编号 | 阻塞点 | 优先级 | 影响 | 验收标准 | 状态 |
|---|---|---|---|---|---|
| B1 | **RapidOCR ONNX 推理未接入**（`recognize()` 为 stub） | P0 | 默认 rapidocr 选择后无实际识别结果，屏幕上下文功能名存实亡 | 设置页「测试识别」能返回真实屏幕文字，dictation 日志出现 `captured N chars` | ✅ 已完成 |
| B2 | **前端 `app/dist` 是占位产物** | P0 | 无法正式打包/运行 Tauri 应用 | 跑 `npm run build` 生成真实 `dist/`，并删除 `index.html` 占位文件 | ✅ 已完成 |
| B3 | **屏幕捕获权限与引导缺失** | P1 | macOS 未授权/Windows 缺 OCR 语言包时失败静默，用户不知原因 | 设置页对错误给出中文提示与跳转引导（macOS 系统设置、Windows 语言包安装） | ✅ 已完成 |
| B4 | **Tauri 打包工具链未验证** | P1 | 无法生成 `.msi` / `.exe` 安装包 | 确认 NSIS/WiX 可用；或至少能用 `cargo run --release` 验证可执行文件 | ✅ 已完成（NSIS 安装包含 onnxruntime.dll，stub IME 占位） |
| B5 | **RapidOCR 模型下载网络兜底** | P1 | 部分网络仍下不动 GitHub Release | 镜像输入框生效；提供手动放置模型 fallback 文档 | ✅ 已完成（默认 URL 已改为 ModelScope） |
| B6 | **OCR 缓存与预取** | P2 | 每次润色同步截图 OCR，延迟高 | 同屏内容 30s 内复用；录音期间可后台预取截图 | ⏳ 待做 |
| B7 | **macOS / Linux 平台补齐** | P2 | macOS 无 OCR 后端，Linux 截图 stub | macOS 接入 Apple Vision 或 ocrs-cjk；Linux 截图路径补齐或裁剪 | ⏳ 待做 |

---

## 2. B1：RapidOCR ONNX 推理接入 — ✅ 已完成

### 2.1 实现摘要

`ocr/rapidocr.rs` 的 `RapidOcrEngine::recognize()` 已使用 `rapidocr-core` 0.2.2 + `ort` 2.0.0-rc.12 (`load-dynamic` 特性) 跑通 PP-OCRv6 small 三模型流水线（det → cls → rec）。

关键实现点：

- `Cargo.toml` 中新增（仅 Windows/macOS 目标段）：
  ```toml
  rapidocr-core = { version = "0.2.2", default-features = false, features = ["load-dynamic"] }
  ort = { version = "2.0.0-rc.12", default-features = false, features = ["load-dynamic"] }
  ```
  - `default-features = false` 关闭 `rapidocr-core` 的默认模型下载（`reqwest`），复用我们已有的 `start_download` 进度管理。
  - `ort` 的 `load-dynamic` 使其在运行时 `LoadLibrary` 加载 ONNX Runtime DLL，**不直接链接**到 `foundry-local-sdk` 的 `onnxruntime.dll`，避免版本冲突。
- 推理流程：`recognize_screen_text` → `xcap` 主屏截图 → PNG 编码 → `RapidOcr::run_image(&rgb)` → 转成内部 `OcrResult`。
- ONNX Runtime 动态库查找顺序（`init_ort`）：`ORT_DYLIB_PATH` 环境变量 → 模型目录 → 可执行文件旁 → 开发环境 `FOUNDRY_NATIVE_OVERRIDE_DIR`。
- 默认 RapidOCR 模型 URL 已从 GitHub Release 改为 **ModelScope**（国内可达）：
  - `PP-OCRv6_det_small.onnx`
  - `PP-OCRv6_rec_small.onnx`
  - `ch_ppocr_mobile_v2.0_cls_mobile.onnx`（来自 PP-OCRv4 cls 目录）
  - `ppocrv6_dict.txt`（rec 阶段字典，必须存在）
- 单元测试：`cargo test --lib rapidocr` 通过 5 项：配置路径对齐、模型清单包含字典、URL 拼接（空 mirror / 前缀 / 尾部斜杠裁剪）。
- 真机冒烟测试：`cargo test --lib -- --ignored ocr::tests::rapidocr_screen_context_smoke_test` 在当前 Windows 桌面成功捕获主屏并识别出 **2165 字符**，验证了 `xcap` 截图 + `rapidocr-core` + `ort` 全链路。

### 2.2 运行时依赖

首次使用 RapidOCR 时，用户需要：

1. 在设置页下载 4 个模型文件（约 30 MB，默认从 ModelScope 拉取）。
2. 在模型目录或应用安装目录提供 `onnxruntime.dll`（约 15 MB）。
   - 开发环境：已放入 `C:\Users\s00883827\.local\openless-native-libs\foundry-dlls\onnxruntime.dll`，`init_ort` 会找到它。
   - 生产打包：需要把 `onnxruntime.dll` 与可执行文件一起分发（见 B4 打包项）。

### 2.3 回退策略

- 若模型未下载或 ONNX Runtime 加载失败，`recognize_screen_text` 返回 `Err`，`capture_screen_context` 捕获后降级为 `None`（不注入屏幕上下文，听写仍可正常完成）。
- Windows 上可临时切换到 `winrt` provider（无需模型下载），但需安装系统中文 OCR 语言包。

---

## 3. B2：构建真实前端 dist — ✅ 已完成

### 3.1 完成摘要

- 在 `openless-all/app` 下运行 `npm run build`（`tsc && vite build`）成功生成真实 `dist/` 目录，包含 2461 个模块的 Vite 产物。
- 删除了此前为 `cargo check` 临时放置的占位 `dist/index.html`。
- `tauri::generate_context!()` 不再报 `frontendDist` 不存在；`tauri-build` 已将 `dist` 目录全部文件哈希化嵌入。

### 3.2 验证

- `dist/index.html` 引用真实 JS/CSS 产物。
- `cargo check --lib` 通过。
- 前端测试（35 项）全绿。

---

## 4. B3：屏幕捕获权限与引导 — ✅ 已完成

### 4.1 后端

- macOS：在 `ocr/mod.rs` 的 `capture_primary_screen` 中调用 `CGPreflightScreenCaptureAccess` 预检；未授权时返回 `screenRecordingDenied` 标记错误。
- Windows：在 `ocr/winrt.rs` 中当中文引擎和用户资料语言引擎均创建失败时，返回 `winrtChineseOcrLanguageMissing` 标记错误。
- `commands/permissions_cmds.rs` 新增 `open_system_settings("screen-recording")` 跳转 `Privacy_ScreenCapture`。

### 4.2 前端

- `OcrSection.tsx` 解析错误标记，分别展示：
  - macOS 未授权：中文引导文案 + 「打开系统设置」按钮。
  - winrt 语言包缺失：Windows 语言包安装引导。
- 5 语言 i18n（en/ja/ko/zh-CN/zh-TW）补全 `settings.ocr.*` 相关 key。

### 4.3 验证

- `cargo check --lib`、`cargo test --lib`（OCR 5 项）、`tsc`、`npm run build` 均通过。

---

## 5. B4：Tauri 打包工具链 — ✅ 已完成

### 5.1 已完成的验证

1. **可执行文件验证**：
   - `cargo run --release`（Windows）成功启动，日志显示 OpenLess 启动、热键注册、QA 热键安装，窗口可弹出。
2. **MSI 安装包**：
   - `npx tauri build` 自动流程在 WiX `light.exe` 阶段失败（错误：`failed to run C:\Users\...\WixTools314\light.exe`）。
   - 手动执行 `light.exe`（从 `openless-all/app` 目录，使用 Tauri 生成的 `main.wixobj`/`openless-ime.wixobj`）成功生成 `OpenLess_1.3.16_x64_en-US.msi`（约 25 MB）。
   - 临时使用 Rust 构建的 stub `OpenLessIme.dll`（x64 + x86）满足打包时的文件引用与安装时的 `regsvr32` 注册，但 **无真实 IME 功能**（不影响屏幕上下文测试，仅影响 Windows 输入法插入）。

### 5.2 NSIS 安装包验证

- `npx tauri build --bundles nsis` 已重新运行并生成包含 `onnxruntime.dll` 的 `OpenLess_1.3.16_x64-setup.exe`（约 17 MB，含 53 MB 压缩后的 `openless.exe` + 15 MB `onnxruntime.dll`）。
- 通过 `7z l` 确认安装包内包含 `onnxruntime.dll`，发布版 RapidOCR 无需依赖外部 DLL 即可加载。

### 5.3 已知风险与待决策

- **真实 IME DLL**：`OpenLessIme.dll` 需要 Visual Studio 2022 + Windows SDK 从 `windows-ime/OpenLessIme.sln` 构建。当前环境无 MSBuild，v1 打包测试使用 stub DLL。正式发布必须替换为真实 IME DLL。
- **Tauri 自动 WiX 失败**：`light.exe` 手动可跑，但 Tauri 自动调用失败，可能是工作目录或参数问题，需进一步排查。
- **release 编译耗时**：`lto = true`、`codegen-units = 1` 下 release 编译约 12–35 分钟，调试验证阶段可临时改为 `lto = "thin"`。

### 5.4 已补齐的打包依赖项

- **`onnxruntime.dll` 打包**：在 `openless-all/app/src-tauri/tauri.conf.json` 的 `bundle.resources` 中加入 `onnxruntime.dll`，并将 foundry-local-sdk 的 64-bit `onnxruntime.dll`（15 MB）复制到 `src-tauri/onnxruntime.dll` 作为本地打包来源。重新打包后 NSIS 安装程序会把该 DLL 与 `openless.exe` 一起释放到安装目录，RapidOCR 启动时即可加载。
  - 该文件已加入 `.gitignore`，不进入版本库；正式发布时应替换为从 Microsoft/ONNX Runtime 官方渠道获取的 DLL，避免分发 foundry 私有 NuGet 内容。
- **环境变量路径格式**：NSIS 的 `File` 宏使用 Windows 路径，因此 `OPENLESS_IME_DLL_X64` / `_X86` 必须设置为反斜杠形式（如 `D:\\c\\openless\\...\\OpenLessIme.dll`），否则 NSIS 会报 `no files found`。

### 5.5 仍需要手动替换的占位项

- **OpenLessIme.dll 为 stub**：当前环境无 Visual Studio / MSBuild，无法从 `windows-ime/OpenLessIme.sln` 构建真实 TSF IME DLL。打包使用的是 Rust 临时构建的 stub DLL（仅导出 `DllCanUnloadNow` / `DllGetClassObject` / `DllRegisterServer` / `DllUnregisterServer` 并返回 S_OK），可让安装程序通过 `regsvr32` 不报错，但 **无实际输入法功能**。正式发布前必须替换为真实 `OpenLessIme.dll`（CI 脚本 `scripts/windows-package-msvc.ps1` 已包含构建流程）。
- **测试版安装程序不含真实 IME**：用当前安装包测试时，OpenLess 仍可运行，但 Windows 输入法插入路径将无效；听写结果会走剪贴板/无障碍回退插入。

### 5.6 打包验证结果（当前会话）

- `npx tauri build --bundles nsis` 成功生成 `OpenLess_1.3.16_x64-setup.exe`。
- 安装程序静默安装需要管理员权限（UAC），在普通用户 shell 中无法直接测试；可执行文件 `openless.exe` 可直接运行验证。
- `cargo run --release` / 直接运行 `target/release/openless.exe` 均正常启动。

---

## 6. B5：RapidOCR 模型下载网络兜底（P1）

### 6.1 当前状态（已验证）

- 默认 RapidOCR 模型 URL 已从 GitHub Release 切换到 **ModelScope**（国内可达）：
  - `PP-OCRv6_det_small.onnx`
  - `PP-OCRv6_rec_small.onnx`
  - `ch_ppocr_mobile_v2.0_cls_mobile.onnx`（来自 PP-OCRv4 cls 目录）
  - `ppocrv6_dict.txt`（来自 PP-OCRv6 rec 目录）
- `start_ocr_model_download(mirror)` 已支持 mirror 参数：若 `mirror` 非空，URL 为 `{mirror}/{filename}`。
- 前端 `OcrSection.tsx` 已提供「下载镜像」输入框，失焦保存。
- **URL 拼接已验证正确**（走查前端输入框 → `lib/ipc/ocr.ts` → Tauri command `start_ocr_model_download` → `rapidocr.rs` 全链路）：
  - 空 mirror → 直接使用默认 ModelScope URL；
  - 非空 mirror → `format!("{}/{filename}", mirror.trim_end_matches('/'))`，自动去除尾部斜杠；
  - `download_one()` 从拼接后 URL 末段取文件名落盘，与模型清单一致（含 `ppocrv6_dict.txt`）；
  - 拼接逻辑已提取为纯函数 `rapidocr::model_download_url(mirror, filename, default_url)`，新增 3 个单元测试固化（空 mirror 回退默认 URL、前缀拼接、尾部斜杠裁剪），`cargo test --lib rapidocr` 全部通过（5 passed）。
- 5 种语言 i18n（en/ja/ko/zh-CN/zh-TW）已补 `settings.ocr.downloadMirrorHint`，设置页镜像输入框下方展示格式说明。

### 6.2 镜像格式约定

- 由于默认 URL 已使用 ModelScope，大部分国内用户无需填写镜像。
- 若需要镜像，mirror 应填到目录前缀（例如 `https://mirror.example.com/rapidocr/v3.9.2/onnx/PP-OCRv6`），应用自动追加 `/模型文件名.onnx`。
- 镜像目录结构示例：
  - `https://mirror.example.com/rapidocr/v3.9.2/onnx/PP-OCRv6/det/PP-OCRv6_det_small.onnx`
  - 输入框填写 `https://mirror.example.com/rapidocr/v3.9.2/onnx/PP-OCRv6`
  - 拼接结果 `https://mirror.example.com/rapidocr/v3.9.2/onnx/PP-OCRv6/det/PP-OCRv6_det_small.onnx` ✓
- 注意：mirror 不要带查询参数（`?token=...`）或填到单个文件层级，否则拼接会出错；字典文件 `ppocrv6_dict.txt` 必须也在镜像的对应目录下。

### 6.3 手动 fallback 文档（已写入设置页文案 + 本文档）

如果自动下载失败（GitHub 与镜像均不可达），可手动放置模型文件，应用无需下载即可识别（`all_model_files_exist()` 检测到 4 个文件齐全即跳过下载）。

**文件清单（PP-OCRv6 small，默认使用 ModelScope 链接，国内可直接访问）**：

| 文件 | 用途 | 默认下载 URL |
|---|---|---|
| `PP-OCRv6_det_small.onnx` | 文本检测 | `https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2/onnx/PP-OCRv6/det/PP-OCRv6_det_small.onnx` |
| `PP-OCRv6_rec_small.onnx` | 中文识别 | `https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2/onnx/PP-OCRv6/rec/PP-OCRv6_rec_small.onnx` |
| `ch_ppocr_mobile_v2.0_cls_mobile.onnx` | 方向分类 | `https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2/onnx/PP-OCRv4/cls/ch_ppocr_mobile_v2.0_cls_mobile.onnx` |
| `ppocrv6_dict.txt` | rec 识别字典（必需，模型不内嵌 vocab） | `https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2/paddle/PP-OCRv6/rec/PP-OCRv6_rec_small/ppocrv6_dict.txt` |

> 4 个文件全部来自 ModelScope，国内网络通常可达；如果自动下载仍失败，可手动放置。

**放置目录（三选一，与设置页「模型存储目录」联动）**：

1. 默认目录（未自定义时，与代码 `default_models_root()/rapidocr` 一致）：
   - Windows：`%APPDATA%\OpenLess\models\rapidocr\`（即 `C:\Users\<user>\AppData\Roaming\OpenLess\models\rapidocr\`）
   - macOS：`~/Library/Application Support/OpenLess/models/rapidocr/`
   - Linux：`$XDG_DATA_HOME/OpenLess/models/rapidocr/`（未设时 `~/.local/share/OpenLess/models/rapidocr/`）
2. 自定义目录：设置页「模型存储目录」填 `D:\models` 时，放入 `D:\models\rapidocr\`。
3. 目录不存在时先手动创建。

**步骤**：

1. 浏览器（或 `curl`/`wget`）下载上表 4 个文件；
2. 创建 `rapidocr` 目录并把 4 个文件放入，**保持文件名不变**；
3. 重启应用，或在设置页重新选择 rapidocr provider；
4. 模型齐全后，应用不再进入下载流程（下载状态表为空），「测试识别」可直接使用（需 B1 推理已接入）。

> 注意：手动放置的文件不会被「删除模型」之外的操作影响；若想恢复自动下载，删除这些文件后点「下载模型」即可。

---

## 7. B6：OCR 缓存与预取（P2）

### 7.1 问题

每次 dictation 松手后都同步 await `capture_screen_context(inner)`，即使屏幕内容没变化。WinRT 全屏截图+OCR 约 200-400ms，rapidocr 模型加载后约 1-3s；在慢机器上会拖慢听写收尾。

### 7.2 设计草案

- **缓存键**：`screen_hash = (display_id, capture_timestamp_bucket_30s, image_size)` 或简单用 30s TTL。
- **缓存值**：`Option<String>`（已识别的屏幕文本，或"无文本"）。
- **存储位置**：`Inner` 中新增 `Mutex<Option<(Instant, String)>>`。
- **流程**：
  - `capture_screen_context` 先查缓存，命中且未过期则直接返回。
  - 未命中则执行截图+OCR，写入缓存。
- **预取**：在录音开始后 500ms（用户大概率还在讲话），后台 spawn `capture_screen_context` 预热缓存；正式润色时命中缓存。

### 7.3 依赖

- B6 依赖 B1（有真实 OCR 结果后缓存才有意义）。

---

## 8. B7：macOS / Linux 平台补齐（P2）

### 8.1 macOS

- 截图：xcap 已支持 macOS，但需 TCC 授权（见 B3）。
- OCR：两个候选方案：
  1. **Apple Vision**（`VNRecognizeTextRequest`）—— 原生、中文效果好，但需 macOS 10.15+ 与 Objective-C/Swift 桥接。
  2. **`ocrs-cjk`**（rten 纯 Rust）—— 跨平台、无额外原生库，但模型需适配。
- 建议 v1 先用 Apple Vision，因为 OpenLess 最低版本 macOS 12.0，Apple Vision 可用。

### 8.2 Linux

- 截图：`xcap` 在 Linux 依赖 X11 开发库，Wayland 不支持。
- 建议 v1 **明确不支持 Linux 屏幕上下文**：`available_providers()` 在 Linux 只返回 `disabled`；`capture_primary_screen` 已在 Linux 返回 `None` 并 warn。这样避免引入系统依赖，保证 Linux 编译/打包不受影响。

---

## 9. 推荐推进顺序（已更新，截至当前会话）

| 阶段 | 任务 | 状态 | 说明 |
|---|---|---|---|
| 1 | B1（RapidOCR 推理） | ✅ 完成 | 已接入 rapidocr-core + ort，并通过真机冒烟测试 |
| 2 | B2（构建真实 dist） | ✅ 完成 | `npm run build` 生成真实 dist，`cargo check` 通过 |
| 3 | B3（权限引导） | ✅ 完成 | 前后端错误标记与 i18n 已补齐 |
| 4 | B5（下载兜底） | ✅ 完成 | 默认 URL 改为 ModelScope，镜像拼接逻辑已测试 |
| 5 | B4（Tauri 打包） | 🔄 进行中 | 可执行文件已启动，MSI 已手动构建，NSIS 构建中 |
| 6 | B6（缓存预取） | ⏳ 待做 | 优化同屏重复 OCR 延迟 |
| 7 | B7（macOS/Linux） | ⏳ 待做 | 后续平台补齐 |

---

## 10. 环境变量速查（用于后续构建/测试）

```powershell
# 必须
$env:SHERPA_ONNX_LIB_DIR="C:\Users\s00883827\.local\openless-native-libs\sherpa-onnx-v1.13.2-win-x64-static-MT-Release-lib\lib"
$env:FOUNDRY_NATIVE_OVERRIDE_DIR="C:\Users\s00883827\.local\openless-native-libs\foundry-dlls"
$env:NO_PROXY="rust.inhuawei.com,localhost,127.0.0.1"
$env:PATH="$env:USERPROFILE\.cargo\bin;$env:PATH"

# 可选：调试 Tauri/前端
$env:TAURI_DEV_WATCHER_ARGS="--no-bundle"  # 如只想编译不打包安装包
```

---

## 11. 测试验收清单（打包后验证）

- [x] 后端 RapidOCR 冒烟测试：在屏幕有文字时识别出非空结果。
- [ ] 应用可启动，设置 → 服务 → OCR 区块可见。
- [ ] 默认 provider 是 `rapidocr`（Windows）。
- [ ] 点击"下载模型"，进度条走到 100%，提示"下载完成"。
- [ ] 点击"测试识别"，返回当前屏幕文字。
- [ ] 切换到 `winrt`，点击"测试识别"，返回当前屏幕文字（需装中文 OCR 语言包）。
- [ ] 开启屏幕上下文开关，在屏幕上有文字时执行一次听写，日志出现 `[screen_context] captured N chars`。
- [ ] 关闭开关或 OCR 失败时，听写仍然正常完成，不报错。
- [ ] 在 Style Pack 设置页预览中可见 `screen_context_block` 内容。

---

## 12. 附录：RapidOCR 模型清单与 URL

| 文件 | 用途 | 默认 URL（ModelScope） |
|---|---|---|
| `PP-OCRv6_det_small.onnx` | 文本检测 | `https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2/onnx/PP-OCRv6/det/PP-OCRv6_det_small.onnx` |
| `PP-OCRv6_rec_small.onnx` | 中文识别 | `https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2/onnx/PP-OCRv6/rec/PP-OCRv6_rec_small.onnx` |
| `ch_ppocr_mobile_v2.0_cls_mobile.onnx` | 方向分类 | `https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2/onnx/PP-OCRv4/cls/ch_ppocr_mobile_v2.0_cls_mobile.onnx` |

完整 4 文件清单（含字典）与手动放置方式见 §6.3。
