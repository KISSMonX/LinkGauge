# iperf3 GUI 测试工具

基于 Rust、Tauri 2、Vue 3 和 TypeScript 的跨平台 iperf3 图形界面。界面按 `GUI.png` 实现，支持 TCP/UDP/Ping 测试队列、实时指标曲线、日志持久化、中断恢复及 HTML/PDF 报告。

## 环境要求

- Node.js 20+
- Rust stable 与 Tauri 2 对应的系统依赖
- 发布包默认内置 iperf3，最终用户无需另行安装

开发版会优先使用 `src-tauri/resources/iperf3` 中的内置运行时；缺少资源时才回退到系统 `PATH`。仍可在配置 JSON 的 `iperfPath` 中填写自定义完整路径覆盖默认行为。

## 开发与构建

```powershell
npm install
npm run vendor:iperf3:windows
npm run tauri dev
```

生成包含内置 iperf3 的 Windows NSIS 安装包：

```powershell
npm run tauri build
```

Linux 构建机在准备运行时后，可用 `npm run tauri build -- --bundles appimage,deb` 生成 AppImage/DEB。

前端生产构建与后端检查：

```powershell
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml
```

日志保存在系统应用日志目录的 `tests` 子目录；报告保存在应用数据目录的 `reports` 子目录。停止或异常结束的任务使用“未完成”文件名，并可在下次启动后恢复队列。

## 内置 iperf3

Windows x64 运行文件已固定为 iperf3 3.21，下载脚本会校验发布资产 SHA-256，并将二进制、Cygwin 运行库和许可证写入 Tauri resources。Linux 构建机可运行：

```sh
./scripts/vendor-iperf3-linux.sh 3.21
```

Linux 脚本从 ESnet 源码构建静态程序，需要 `curl`、`tar`、C 编译工具链和静态 libc 开发包。第三方来源与许可证记录见 `src-tauri/resources/iperf3/THIRD-PARTY-NOTICES.md`。
