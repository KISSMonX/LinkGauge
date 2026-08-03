# iperf3 GUI

[English](README.md) | [简体中文](README.zh-CN.md)

一款基于 Rust、Tauri 2、Vue 3 和 TypeScript 开发的桌面网络性能测试工具。软件为 Ping、TCP 和 UDP 测试提供工程化图形流程，并由 Rust 后端负责测试执行、进程控制和日志持久化。

> 当前发布状态：Windows x64 已支持生成内置 iperf3 3.21 的 NSIS 安装包。Linux 支持从源码构建，需要在 Linux 构建机上准备对应的内置运行时。

![iperf3 GUI](doc/GUI.png)

## 功能特性

- 客户端和服务端两种运行模式
- TCP、UDP 参数分离展示
- Ping 连通性测试
- TCP 单向、双向、多并发流、Reverse 和压力测试
- UDP 带宽、抖动和丢包率测试
- 串行测试队列及等待、运行、成功、失败、停止状态
- 实时带宽曲线和汇总统计
- 本机及对端网络信息展示
- INFO、WARN、ERROR 实时日志与等级筛选
- 按测试任务保存日志，文件名区分完成和未完成状态
- 测试安全中止及未完成队列恢复
- JSON 配置导入、导出和本地保存
- HTML、PDF 测试报告
- 内置 iperf3，并支持系统 `PATH` 和自定义路径回退
- Windows、Linux 构建流程

## 软件架构

```mermaid
flowchart LR
    UI[Vue 3 界面] -->|Tauri invoke| API[Tauri 命令]
    API --> RUNNER[Rust 异步任务执行器]
    RUNNER --> PING[系统 Ping]
    RUNNER --> IPERF[内置 iperf3]
    PING --> NETWORK[(网络对端)]
    IPERF --> NETWORK
    RUNNER -->|test-event| UI
    RUNNER --> LOGS[测试日志文件]
    UI --> REPORT[报告命令]
    REPORT --> OUTPUT[HTML / PDF 报告]
```

软件采用 Tauri 双进程模型：

- **前端：** Vue 组件负责参数配置、数据面板、任务队列、日志、曲线、弹窗和报告概览。`src/App.vue` 负责任务编排和可恢复状态持久化。
- **后端：** Rust 负责校验请求、解析 iperf3 运行时、异步启动子进程、解析输出、发送事件、保存日志和生成报告。
- **进程通信：** 前端仅调用有限的 Tauri 命令，并接收 `test-event` 更新。命令参数构造和子进程所有权保留在 Rust 侧。
- **运行时解析：** 自定义可执行文件路径优先；否则选择当前平台的内置二进制。内置资源不可用时，才回退到系统 `PATH` 中的 `iperf3`。

### 后端命令

| 命令 | 职责 |
| --- | --- |
| `start_test` | 校验配置并启动 Ping 或 iperf3 任务 |
| `stop_test` | 发出取消信号并终止当前子进程 |
| `get_network_info` | 读取本机 IP、MAC 地址和主机名 |
| `get_iperf_runtime_info` | 查找并验证内置或外部 iperf3 运行时 |
| `generate_report` | 在应用数据目录中生成 HTML 或 PDF 报告 |

## 项目结构

```text
.
├── doc/                         # 参考界面和功能设计说明
├── scripts/                     # 各平台 iperf3 准备脚本
├── src/                         # Vue 3 前端
│   ├── components/              # 面板、曲线、工具栏和通用图标
│   ├── App.vue                  # 应用状态和测试队列编排
│   ├── styles.css               # 桌面布局与视觉样式
│   └── types.ts                 # 前端数据契约
├── src-tauri/
│   ├── resources/iperf3/        # 内置运行时与第三方声明
│   ├── src/
│   │   ├── models.rs            # IPC 与领域模型
│   │   ├── runner.rs            # 异步执行、解析、中止和日志
│   │   ├── runtime.rs           # 内置运行时查找与验证
│   │   ├── report.rs            # HTML/PDF 报告生成
│   │   └── system.rs            # 本机网络信息
│   ├── Cargo.toml               # Rust 依赖
│   └── tauri.conf.json          # 窗口、资源和安装包配置
├── package.json                 # 前端依赖和 npm 命令
└── vite.config.ts               # Vite 开发与构建配置
```

## 项目依赖

### 应用技术栈

| 层级 | 主要依赖 | 用途 |
| --- | --- | --- |
| 桌面框架 | Tauri 2 | 原生窗口、IPC、系统路径、资源和安装包 |
| 前端 | Vue 3、TypeScript、Vite | 界面、应用状态和生产构建 |
| 曲线 | Chart.js、vue-chartjs | 实时带宽可视化 |
| 异步运行时 | Tokio | 子进程、文件 I/O、中止和事件循环 |
| 序列化 | Serde、serde_json | 前后端类型化数据及配置 |
| 输出解析 | regex | 解析 iperf3 和 Ping 输出 |
| 系统信息 | hostname、local-ip-address、mac_address | 本机网络标识 |
| 工具库 | chrono、uuid | 时间戳、文件名和会话 ID |
| 测试引擎 | iperf3 3.21 | TCP/UDP 网络性能测量 |

JavaScript 和 Rust 依赖的确切约束分别记录在 `package-lock.json` 和 `src-tauri/Cargo.lock`。

## 开发环境要求

### Windows

- Windows 10 或 Windows 11 x64
- Node.js 20 或更高版本
- Rust stable 和 MSVC 工具链
- Microsoft C++ Build Tools，并安装 **Desktop development with C++**
- Microsoft Edge WebView2 Runtime

最新平台要求以 [Tauri 官方环境准备说明](https://v2.tauri.app/zh-cn/start/prerequisites/) 为准。

### Debian/Ubuntu

安装 Tauri 2 系统依赖：

```bash
sudo apt update
sudo apt install -y \
  libwebkit2gtk-4.1-dev \
  build-essential \
  curl wget file \
  libxdo-dev libssl-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev
```

随后安装 Node.js 20+ 和 Rust stable。构建静态 Linux iperf3 运行时可能还需要当前发行版提供的静态 libc 开发包。

## 快速开始

克隆仓库并安装 JavaScript 依赖：

```bash
git clone <repository-url>
cd iperf3-gui
npm ci
```

Windows 运行时已保存在 `src-tauri/resources/iperf3/windows-x86_64`。如需重新下载或升级，可运行带固定校验值的脚本：

```powershell
npm run vendor:iperf3:windows
```

启动 Tauri 开发应用：

```bash
npm run tauri dev
```

仅启动浏览器前端可运行 `npm run dev`。浏览器预览模式无法调用原生命令，因此会使用模拟测试数据。

## 构建

### 前端和后端检查

```bash
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
```

### Windows NSIS 安装包

```powershell
npm ci
npm run vendor:iperf3:windows
npm run tauri build
```

安装包输出到：

```text
src-tauri/target/release/bundle/nsis/iperf3 GUI 测试工具_<version>_x64-setup.exe
```

安装包包含 `iperf3.exe`、`cygwin1.dll` 和所需第三方许可证，最终用户无需单独安装 iperf3。

### Linux AppImage 和 DEB

在 Linux 构建机上准备静态 iperf3：

```bash
sh scripts/vendor-iperf3-linux.sh 3.21
```

构建安装包：

```bash
npm ci
npm run tauri build -- --bundles appimage,deb
```

Linux 产物应在计划支持的最旧基础发行版上构建，以避免引入过新的 glibc 要求。详见 [Tauri AppImage 指南](https://v2.tauri.app/distribute/appimage/)。

## 安装

### Windows

1. 从项目发布产物中下载 `*_x64-setup.exe`。
2. 如果发布页提供校验值，请先验证文件哈希。
3. 运行安装程序并完成 NSIS 安装向导。
4. 从开始菜单启动 **iperf3 GUI 测试工具**。

当前安装包尚未进行代码签名，本地构建或未正式发布的安装包可能触发 Windows SmartScreen 提示。

### Linux

- **AppImage：** 添加可执行权限后运行。

  ```bash
  chmod +x iperf3-gui_*.AppImage
  ./iperf3-gui_*.AppImage
  ```

- **Debian/Ubuntu：** 安装 DEB 包。

  ```bash
  sudo apt install ./iperf3-gui_*.deb
  ```

## 使用方法

1. 选择客户端或服务端模式。
2. 选择 TCP 或 UDP，并勾选需要执行的测试项目。
3. 客户端模式下填写服务端地址、端口、测试时间及协议参数。
4. 开始测试，在任务队列、实时曲线、统计信息和日志区域查看执行状态。
5. 必要时停止测试。软件会保留部分日志，并在下次启动时恢复剩余队列。
6. 一个或多个项目完成后，可生成 HTML 或 PDF 报告。

客户端测试要求对端可达、防火墙允许配置的 TCP/UDP 端口，并且对端已有 iperf3 服务端运行。

## 配置与数据

- 配置支持 JSON 导入和导出。
- “保存配置”将当前设置写入本地 WebView 存储。
- `iperfPath` 默认为 `bundled`，可将其设置为绝对路径以覆盖内置运行时。
- 中断恢复状态保存在本地；全部队列成功后自动清除。
- 测试日志保存在 Tauri 对应操作系统的应用日志目录下的 `tests/`。
- 报告保存在 Tauri 对应操作系统的应用数据目录下的 `reports/`。

日志文件名格式：

```text
<本机IP>-<服务端IP>-<测试名称>-<yyyyMMddHHmmss>-<完成|未完成>.log
```

## 内置 iperf3 与供应链

- 版本：iperf3 3.21
- Windows 架构：x86_64
- Windows 二进制来源：[ar51an/iperf3-win-builds](https://github.com/ar51an/iperf3-win-builds)
- 上游源码：[ESnet/iperf](https://github.com/esnet/iperf)
- Windows 下载脚本固定发布资产，并在解压前校验 SHA-256。
- 运行文件和许可证作为 Tauri resources 打入安装包。

ESnet 官方支持 Linux、macOS 和 FreeBSD；内置 Windows 二进制属于社区构建。再分发前请检查 `src-tauri/resources/iperf3/THIRD-PARTY-NOTICES.md`。

## 常见问题

| 现象 | 建议处理方式 |
| --- | --- |
| 运行时显示不可用 | 重新运行内置资源脚本，或将 `iperfPath` 设置为有效路径 |
| 服务端无响应 | 检查地址、端口、防火墙、路由和服务端模式 |
| 没有实时采样 | 确认两端 iperf3 版本兼容，且进程输出周期数据 |
| Linux 应用无法启动 | 检查 WebKitGTK 4.1 和发行版运行依赖 |
| Windows 构建无法替换 EXE | 关闭正在运行的 `iperf3-gui.exe` 后重新构建 |
| SmartScreen 告警 | 使用可信代码签名证书签署正式安装包 |

## 参与贡献

仓库公开后欢迎提交 Issue 和 Pull Request。

1. 创建职责单一的开发分支。
2. 修改数据结构时，保持 `src/types.ts` 与 Rust 模型一致。
3. 提交前运行前端构建、Rust 测试和格式检查。
4. 不要提交 `node_modules`、`dist` 或 `src-tauri/target`。
5. 替换第三方二进制时，必须同时更新校验值和第三方声明。

## 许可证

当前仓库根目录尚无项目级 `LICENSE` 文件。正式作为开源项目发布前，版权持有人需要选择并添加许可证，同时更新本节和包元数据。在没有明确项目许可证的情况下，应用原创源代码仍受默认版权限制。

第三方组件继续遵循其各自许可证：

- iperf3：BSD-3-Clause
- Windows iperf3 构建仓库：Apache-2.0
- JavaScript 和 Rust 依赖：各上游项目声明的许可证

内置运行时的完整再分发声明位于 [`src-tauri/resources/iperf3/THIRD-PARTY-NOTICES.md`](src-tauri/resources/iperf3/THIRD-PARTY-NOTICES.md)，完整许可证文本保存在二进制文件旁。

## 致谢

- [ESnet iperf3](https://github.com/esnet/iperf)：网络性能测试引擎
- [Tauri](https://tauri.app/)：桌面应用框架
- [Vue](https://vuejs.org/) 与 [Chart.js](https://www.chartjs.org/)：前端和数据可视化技术栈
