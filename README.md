# LinkGauge

[English](README.md) | [中文](README.zh-CN.md)

A desktop network performance testing application built with Rust, Tauri 2, Vue 3, and TypeScript. It provides a structured GUI workflow for Ping, TCP, and UDP testing. TCP/UDP tests run on a pure-Rust, in-process [riperf3](https://github.com/therealevanhenry/riperf3) engine that speaks the iperf3 wire protocol — no iperf3 binary is installed, bundled, or spawned; only Ping uses the system command.

> Current release status: Windows x64 is fully packaged as an NSIS installer. Linux builds are supported from source. The installer contains no third-party network-testing binaries.

![LinkGauge](doc/GUI.png)

## Features

- Client and server operating modes
- Separate TCP and UDP configuration views
- Ping connectivity checks
- TCP single-direction, bidirectional, parallel-stream, reverse, and stress tests
- UDP bandwidth, jitter, and packet-loss tests
- Sequential test queue with waiting, running, successful, failed, and stopped states
- Live bandwidth chart and aggregate statistics
- Local and peer network information
- Multi-NIC detection with an interface picker (the first interface is the default) and link-speed reporting
- Bandwidth presets (100 / 1000 Mbps, unlimited) that default to the current NIC link speed
- Packet-length presets from 128 bytes to 64 KB, with a custom length persisted to the config file
- Real-time INFO, WARN, and ERROR logs with filtering
- Per-test log files with completed/incomplete status in the filename
- Graceful test cancellation and unfinished queue recovery
- JSON configuration import, export, and local persistence
- HTML and PDF report generation
- Pure-Rust riperf3 engine: interoperable with standard iperf3 servers, no runtime dependencies
- Windows and Linux build workflows

## Architecture

```mermaid
flowchart LR
    UI[Vue 3 UI] -->|Tauri invoke| API[Tauri commands]
    API --> RUNNER[Rust async task runner]
    RUNNER --> PING[System Ping]
    RUNNER --> ENGINE[riperf3 in-process engine]
    PING --> NETWORK[(Network peer)]
    ENGINE --> NETWORK
    ENGINE -->|on_interval callback| RUNNER
    RUNNER -->|test-event| UI
    RUNNER --> LOGS[Test log files]
    UI --> REPORT[Report command]
    REPORT --> OUTPUT[HTML / PDF reports]
```

The application uses Tauri's two-process model:

- **Frontend:** Vue components render the configuration, dashboard, task queue, logs, chart, dialogs, and report summary. `src/App.vue` coordinates the test queue and persists recoverable state.
- **Backend:** Rust validates requests, drives the in-process riperf3 engine, streams per-interval metrics through the `on_interval` callback, emits typed events, saves logs, and creates reports.
- **IPC:** The frontend invokes a small command surface and receives `test-event` updates. Test execution and result aggregation remain entirely in Rust.
- **Engine:** [riperf3](https://github.com/therealevanhenry/riperf3) is a ground-up, wire-compatible Rust implementation of iperf3. It is vendored under `vendor/riperf3` with a small local patch (a live `on_interval` callback) — see [Test Engine](#test-engine-riperf3). Because the engine runs inside the application process, there is no external binary to resolve, spawn, or manage, and per-second metrics arrive through typed callbacks instead of output parsing.

### Backend commands

| Command | Responsibility |
| --- | --- |
| `start_test` | Validate configuration and run a Ping or riperf3 client/server task |
| `stop_test` | Signal cancellation: kill the Ping process or gracefully interrupt the riperf3 run |
| `get_network_info` | Read the local IP address, MAC address, hostname, and link speed |
| `get_network_interfaces` | Enumerate all up IPv4 interfaces with MAC address and link speed |
| `get_custom_packet_length` | Read the persisted custom packet length from the settings file |
| `save_custom_packet_length` | Validate and persist a custom packet length to the settings file |
| `generate_report` | Generate an HTML or PDF report in the application data directory |

## Project Structure

```text
.
├── doc/                         # Reference UI and functional specification
├── src/                         # Vue 3 frontend
│   ├── components/              # UI panels, chart, toolbar, and shared icons
│   ├── App.vue                  # Application state and task queue orchestration
│   ├── styles.css               # Desktop layout and visual system
│   └── types.ts                 # Frontend data contracts
├── src-tauri/
│   ├── src/
│   │   ├── models.rs            # IPC and domain models
│   │   ├── runner.rs            # riperf3 client/server tasks, Ping, logs, cancellation
│   │   ├── report.rs            # HTML/PDF report generation
│   │   ├── settings.rs          # Settings file read and persistence
│   │   └── system.rs            # Local network information
│   ├── Cargo.toml               # Rust dependencies
│   └── tauri.conf.json          # Desktop window and bundle configuration
├── vendor/riperf3/              # Vendored riperf3 library (MIT OR Apache-2.0) + local patch
├── package.json                 # Frontend dependencies and npm scripts
└── vite.config.ts               # Vite development/build configuration
```

## Dependencies

### Application stack

| Layer | Main dependencies | Purpose |
| --- | --- | --- |
| Desktop shell | Tauri 2 | Native window, IPC, paths, and installers |
| Frontend | Vue 3, TypeScript, Vite | UI, application state, and production bundling |
| Charts | Chart.js, vue-chartjs | Live bandwidth visualization |
| Async runtime | Tokio | Async tasks, file I/O, cancellation, and event loops |
| Serialization | Serde, serde_json | Typed frontend/backend payloads and configuration |
| Parsing | regex | Ping output parsing |
| System information | hostname, local-ip-address, mac_address | Local network identity |
| Utility | chrono, uuid | Timestamps, filenames, and session IDs |
| Test engine | riperf3 (vendored, pure Rust) | TCP/UDP network performance measurement |

Exact JavaScript and Rust dependency constraints are recorded in `package-lock.json` and `src-tauri/Cargo.lock`.

## Prerequisites

### Windows development

- Windows 10 or Windows 11 x64
- Node.js 20 or later
- Rust stable with the MSVC toolchain
- Microsoft C++ Build Tools with **Desktop development with C++**
- Microsoft Edge WebView2 Runtime

See the official [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for current platform requirements.

### Debian/Ubuntu development

Install the Tauri 2 system packages:

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

Then install Node.js 20+ and Rust stable.

## Getting Started

Clone the repository and install JavaScript dependencies:

```bash
git clone git@github.com:KISSMonX/LinkGauge.git
cd LinkGauge
npm ci
```

Start the Tauri development application:

```bash
npm run tauri dev
```

To run only the browser frontend, use `npm run dev`. The browser-only mode uses simulated test data because native commands are available only inside Tauri.

## Build

### Frontend and backend checks

```bash
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
```

### Windows NSIS installer

```powershell
npm ci
npm run tauri build
```

The installer is written to:

```text
src-tauri/target/release/bundle/nsis/LinkGauge_<version>_x64-setup.exe
```

The installer contains no external runtime binaries; the riperf3 engine is compiled into the application.

### Linux AppImage and DEB

Build the packages:

```bash
npm ci
npm run tauri build -- --bundles appimage,deb
```

Build Linux artifacts on the oldest supported base distribution to avoid introducing a newer glibc requirement. See the [Tauri AppImage guidance](https://v2.tauri.app/distribute/appimage/).

## Installation

### Windows

1. Download the generated `*_x64-setup.exe` from the project release artifacts.
2. Verify its checksum when one is published with the release.
3. Run the installer and follow the NSIS wizard.
4. Start **LinkGauge** from the Start menu.

The current installer is not code-signed, so Windows SmartScreen may display a warning for locally built or unpublished packages.

### Linux

- **AppImage:** mark the file executable and run it.

  ```bash
  chmod +x linkgauge_*.AppImage
  ./linkgauge_*.AppImage
  ```

- **Debian/Ubuntu:** install the DEB package.

  ```bash
  sudo apt install ./linkgauge_*.deb
  ```

## Usage

1. Choose **Client** or **Server** mode.
2. Select TCP or UDP and enable the required test items.
3. In client mode, enter the server address, port, duration, and protocol-specific parameters.
4. Start the test and monitor the task queue, live chart, statistics, and logs.
5. Stop a test when necessary. The partial log is retained, and the remaining queue can be recovered on the next launch.
6. Generate an HTML or PDF report after one or more tasks finish.

The peer must be reachable, its firewall must allow the configured TCP/UDP port, and an iperf3 server must be running when the application is used in client mode. The built-in engine is wire-compatible with standard iperf3 servers (and riperf3 servers).

## Configuration and Data

- Configuration can be imported or exported as JSON.
- **Save Configuration** stores the current settings in the local WebView storage.
- Recovery state is stored locally and removed after the complete queue succeeds.
- Test logs are written under the OS-specific Tauri application log directory in `tests/`.
- Reports are written under the OS-specific Tauri application data directory in `reports/`.
- The custom packet length is persisted to `settings.json` in the OS-specific Tauri application config directory.

Log filenames follow this pattern:

```text
<local-ip>-<server-ip>-<test-name>-<yyyyMMddHHmmss>-<completed|incomplete>.log
```

## Test Engine (riperf3)

- Engine: [riperf3](https://github.com/therealevanhenry/riperf3) — a ground-up, wire-compatible Rust implementation of the iperf3 protocol, vendored at `vendor/riperf3` (upstream HEAD, version 0.9.0-dev).
- The engine runs **in-process**: no iperf3 executable is installed, bundled, resolved, or spawned. Per-second metrics flow through typed callbacks; tests can be interrupted gracefully via a watch channel.
- **Local patch:** upstream exposes interval results only after a run completes, so a small `on_interval` callback was added (see `vendor/riperf3` — `IntervalReporterConfig`, `ClientBuilder::on_interval`, `ServerBuilder::on_interval`). The patch is marked with `local LinkGauge patch` comments; re-apply it after upgrading the vendored source.
- Interop: the engine is interoperable with real iperf3 servers and clients (verified upstream against iperf 3.21).
- Known platform difference: TCP retransmission counts depend on `TCP_INFO`, which is unavailable on Windows; the app reports 0 there.

## Troubleshooting

| Symptom | Suggested action |
| --- | --- |
| Server does not respond | Check the address, port, firewall, routing, and server mode |
| No live samples appear | Confirm the peer runs an iperf3-compatible server (iperf3 3.x or riperf3) and the interval is set to 1 s or more |
| Linux application does not start | Verify WebKitGTK 4.1 and distribution runtime dependencies |
| Windows build cannot replace the EXE | Close any running `linkgauge.exe` instance and rebuild |
| SmartScreen warning | Code-sign release installers with a trusted certificate |

## TODO

### Backlog

- [ ] Code-sign release installers to remove the Windows SmartScreen warning
- [ ] Verify Linux AppImage / DEB builds and runtime on the oldest supported base distribution
- [ ] Add a project-level LICENSE file and package metadata
- [ ] Set up CI/CD (e.g. GitHub Actions) for automated builds and releases
- [ ] Test result history and multi-run comparison
- [ ] Unit tests for key frontend logic

## Contributing

Issues and pull requests are welcome after the repository is published.

1. Create a focused branch.
2. Keep UI contracts in `src/types.ts` aligned with Rust models.
3. Run the frontend build, Rust tests, and formatting checks.
4. Do not commit `node_modules`, `dist`, or `src-tauri/target`.
5. When upgrading `vendor/riperf3`, keep the `local LinkGauge patch` (`on_interval`) in sync and update the engine version notes in this README.

## License

This repository does not currently contain a project-wide root `LICENSE` file. Before publishing it as open source, the copyright holder must select and add a license, then update this section and the package metadata. Without an explicit project license, normal copyright restrictions apply to the application's original source code.

Third-party components retain their own licenses:

- riperf3: MIT OR Apache-2.0 (see [`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md) and the license texts in `vendor/riperf3/`)
- JavaScript and Rust dependencies: their respective upstream licenses

## Acknowledgements

- [riperf3](https://github.com/therealevanhenry/riperf3) for the pure-Rust iperf3-compatible engine
- [ESnet iperf3](https://github.com/esnet/iperf) for the wire protocol this tool interoperates with
- [Tauri](https://tauri.app/) for the desktop application framework
- [Vue](https://vuejs.org/) and [Chart.js](https://www.chartjs.org/) for the frontend and visualization stack
