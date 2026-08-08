# OpenLessIme stub DLL（仅本地打包占位）

此目录用于在没有 Visual Studio 2022 / MSBuild 的环境中临时构建一个
满足安装脚本签名的 `OpenLessIme.dll` stub，使 `npx tauri build --bundles nsis`
能够完成打包。

## 注意

- 这个 stub **没有实际 TSF IME 功能**，安装时仅让 `regsvr32 /s` 不报错。
- 正式发布前必须替换为从 `windows-ime/OpenLessIme.sln` 构建的真实 DLL。
- CI 脚本 `scripts/windows-package-msvc.ps1` 已包含真实 DLL 的构建流程。

## 构建

```powershell
# 需要 Rust 工具链支持 i686-pc-windows-msvc 目标
cd openless-all/app/windows-ime/stub
cargo build --release --target x86_64-pc-windows-msvc
cargo build --release --target i686-pc-windows-msvc

# 输出路径供 NSIS 安装脚本使用
$env:OPENLESS_IME_DLL_X64 = "D:\c\openless\openless-all\app\src-tauri\target\windows-ime-msvc\x64\Release\OpenLessIme.dll"
$env:OPENLESS_IME_DLL_X86 = "D:\c\openless\openless-all\app\src-tauri\target\windows-ime-msvc\x86\Release\OpenLessIme.dll"
```
