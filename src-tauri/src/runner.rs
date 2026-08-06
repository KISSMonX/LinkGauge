use crate::models::{MetricPoint, TestEvent, TestRequest};
use chrono::Local;
use regex::Regex;
use riperf3::{ClientBuilder, ServerBuilder, Termination, TransportProtocol};
use std::{
    collections::{HashMap, HashSet},
    fs::File as StdFile,
    io::Write as StdWrite,
    process::Stdio,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, RwLock,
    },
};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::{
    fs::{self, OpenOptions},
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::{mpsc, watch, Mutex as AsyncMutex},
    time::{sleep, Duration},
};
use uuid::Uuid;

/// 按界面语言选择消息文案（locale 为空时默认中文）
fn tr<'a>(locale: &str, zh: &'a str, en: &'a str) -> &'a str {
    if locale == "en" {
        en
    } else {
        zh
    }
}

/// 按界面语言选择格式化模板（模板必须是字面量，format! 才能编译期检查）
macro_rules! tr_format {
    ($locale:expr, $zh:literal, $en:literal $(, $arg:expr)* $(,)?) => {{
        if $locale == "en" {
            format!($en $(, $arg)*)
        } else {
            format!($zh $(, $arg)*)
        }
    }};
}

/// 每个会话的中断信号：ping 走进程取消标志，riperf3 走 watch 通道（优雅终止）
pub(crate) enum SessionSignal {
    Ping(Arc<AtomicBool>),
    Engine(watch::Sender<Option<String>>),
}

#[derive(Default)]
pub struct AppState {
    pub(crate) sessions: Arc<AsyncMutex<HashMap<String, SessionSignal>>>,
    /// ping 子进程残留 PID 集合（riperf3 为进程内引擎，无子进程）
    pub(crate) child_pids: Arc<AsyncMutex<HashSet<u32>>>,
    /// 界面语言（zh / en，空 = zh）：运行时可变，切换语言后运行中会话的引擎日志实时跟随
    pub(crate) locale: Arc<RwLock<String>>,
}

/// 读取当前界面语言（日志输出时调用，避免使用会话启动时的快照）
pub(crate) fn current_locale(handle: &Arc<RwLock<String>>) -> String {
    handle.read().map(|v| v.clone()).unwrap_or_default()
}

/// 切换界面语言：运行中的客户端/服务端会话日志立即改用新语言输出
#[tauri::command]
pub fn set_locale(state: State<'_, AppState>, locale: String) {
    if let Ok(mut guard) = state.locale.write() {
        *guard = locale;
    }
}

/// 应用退出时同步终止所有遗留 ping 测试进程
pub fn kill_all_children_sync(state: &AppState) {
    let pids: Vec<u32> = state.child_pids.blocking_lock().iter().copied().collect();
    for pid in pids {
        #[cfg(windows)]
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .status();
        #[cfg(not(windows))]
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status();
    }
    state.child_pids.blocking_lock().clear();
}

#[tauri::command]
pub async fn start_test(
    app: AppHandle,
    state: State<'_, AppState>,
    request: TestRequest,
) -> Result<String, String> {
    validate(&request)?;
    let session_id = Uuid::new_v4().to_string();
    let sessions = state.sessions.clone();
    let pids = state.child_pids.clone();
    let locale_handle = state.locale.clone();
    let spawned_id = session_id.clone();
    if request.task_id == "ping" {
        let cancelled = Arc::new(AtomicBool::new(false));
        state
            .sessions
            .lock()
            .await
            .insert(session_id.clone(), SessionSignal::Ping(cancelled.clone()));
        tauri::async_runtime::spawn(async move {
            run_ping(
                app,
                spawned_id.clone(),
                request,
                cancelled,
                pids,
                locale_handle,
            )
            .await;
            sessions.lock().await.remove(&spawned_id);
        });
    } else if request.mode == "server" || request.task_id == "server" {
        let (tx, rx) = watch::channel(None);
        state
            .sessions
            .lock()
            .await
            .insert(session_id.clone(), SessionSignal::Engine(tx));
        tauri::async_runtime::spawn(async move {
            run_engine_server(app, spawned_id.clone(), request, rx, locale_handle).await;
            sessions.lock().await.remove(&spawned_id);
        });
    } else {
        let (tx, rx) = watch::channel(None);
        state
            .sessions
            .lock()
            .await
            .insert(session_id.clone(), SessionSignal::Engine(tx));
        tauri::async_runtime::spawn(async move {
            run_engine_client(app, spawned_id.clone(), request, rx, locale_handle).await;
            sessions.lock().await.remove(&spawned_id);
        });
    }
    Ok(session_id)
}

#[tauri::command]
pub async fn stop_test(state: State<'_, AppState>, session_id: String) -> Result<(), String> {
    let map = state.sessions.lock().await;
    let signal = map
        .get(&session_id)
        .ok_or_else(|| "当前测试任务不存在或已经结束".to_string())?;
    match signal {
        SessionSignal::Ping(flag) => flag.store(true, Ordering::SeqCst),
        SessionSignal::Engine(tx) => {
            let _ = tx.send(Some("用户手动停止".into()));
        }
    }
    Ok(())
}

/// 用系统文件管理器打开测试日志目录，返回目录路径
#[tauri::command]
pub async fn open_log_dir(app: AppHandle) -> Result<String, String> {
    let dir = app
        .path()
        .app_log_dir()
        .map_err(|e| e.to_string())?
        .join("tests");
    fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("无法创建日志目录：{e}"))?;
    let result = std::process::Command::new(if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    })
    .arg(&dir)
    .spawn()
    .map_err(|e| format!("无法打开日志目录：{e}"))?;
    drop(result);
    Ok(dir.to_string_lossy().to_string())
}

fn validate(request: &TestRequest) -> Result<(), String> {
    // 服务端模式不需要持续时间（duration 恒为 0）；按量测试（-n/-k）忽略时长。
    // 按量测试项强制 bytes/blocks，与全局 transfer_mode 同源推导
    let transfer_mode = effective_transfer_mode(request);
    if request.mode != "server" && transfer_mode == "time" && request.duration == 0 {
        return Err("持续时间必须大于 0".into());
    }
    if request.interval == 0 {
        return Err("输出周期必须大于 0".into());
    }
    if request.server_ip.trim().is_empty() && request.mode != "server" {
        return Err("服务端地址不能为空".into());
    }
    if request.port == 0 {
        return Err("端口无效".into());
    }
    // 服务端认证依赖私钥与用户文件两个路径，缺一不可（引擎在两者均提供时才校验凭据）
    if request.mode == "server"
        && request.server_auth_enabled
        && (request.server_auth_private_key_path.trim().is_empty()
            || request.server_auth_users_path.trim().is_empty())
    {
        return Err("启用认证后，RSA 私钥与授权用户文件路径均不能为空".into());
    }
    // 预热与按量测试互斥（iperf3 CLI 同样拒绝 -O + -n/-k）；
    // 按时长模式下预热必须落在测试时长内，否则统计区间为空
    if request.omit_secs > 0 {
        if transfer_mode != "time" {
            return Err("预热（-O）仅支持按时长模式".into());
        }
        if u64::from(request.omit_secs) >= request.duration {
            return Err("预热时间必须小于测试时长".into());
        }
    }
    // 套接字缓冲区上限（KB）：与界面输入框上限一致，防溢出（引擎按 i32 字节接收）
    if request.window_kb > 16384 {
        return Err("套接字缓冲区不能超过 16MB".into());
    }
    // IP 协议族只接受 0（自动）/ 4 / 6，其余取值直接拒绝
    if !matches!(request.ip_version, 0 | 4 | 6) {
        return Err("IP 协议族只能是 0（自动）、4 或 6".into());
    }
    // 服务端防护参数范围（与界面输入框上限一致，0 = 不限制）
    if request.server_idle_timeout > 86400 {
        return Err("服务端空闲超时不能超过 86400 秒".into());
    }
    if request.server_max_duration > 86400 {
        return Err("单次测试最大时长不能超过 86400 秒".into());
    }
    if request.server_bitrate_limit_mbps > 1_000_000 {
        return Err("服务端带宽上限不能超过 1000000 Mbps".into());
    }
    // 结束条件与按量数量：time / bytes / blocks 三选一，按量必须大于 0
    match transfer_mode {
        "time" => {}
        "bytes" | "blocks" if request.transfer_amount > 0 => {}
        "bytes" | "blocks" => return Err("按量测试的传输量必须大于 0".into()),
        _ => return Err("测试结束条件只能是 time / bytes / blocks".into()),
    }
    // DSCP 范围 0-63（引擎 parse_dscp 同样约束，超出会被拒绝）
    if request.dscp > 63 {
        return Err("DSCP 值应在 0-63 之间".into());
    }
    // 拥塞控制仅 Linux/FreeBSD 支持；其他平台引擎在 build() 直接报
    // Unsupported，这里提前给出更友好的提示（macOS 等 unix 平台放行）
    #[cfg(not(unix))]
    if !request.congestion_algo.trim().is_empty() {
        return Err("拥塞控制算法（-C）仅支持 Linux/FreeBSD".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// riperf3 客户端任务
// ---------------------------------------------------------------------------

/// 客户端参数映射（纯函数，便于单测）——与旧 CLI 参数一一对应
#[derive(Debug, PartialEq)]
struct ClientParams {
    protocol: TransportProtocol,
    duration: u32,
    num_streams: u32,
    blksize: Option<usize>,
    reverse: bool,
    bidir: bool,
    /// 带宽限制（bps），0 = 不限制。必须无条件下发给引擎，不能「为 0 就不设置」：
    /// riperf3 未调用 bandwidth() 时 UDP 会套用 iperf3 的 UDP_RATE 默认值
    /// （1 Mibit/s，见 vendor/riperf3 的 utils.rs），导致界面选「不限制」的 UDP
    /// 测试实际被限速到约 1 Mbps；显式传 0 才是引擎语义下的不限制。
    bandwidth_bps: u64,
    interval: f64,
    bind_address: Option<String>,
    /// 预热秒数（0 = 不预热）：on_interval 回调已跳过 omitted 区间，仅影响统计口径
    omit_secs: u32,
    /// 套接字缓冲区（KB，0 = 自动）
    window_kb: u32,
    /// 数据流源端口（0 = 自动；第 i 条流绑定 cport+i，对应 iperf3 --cport）
    cport: u16,
    /// IP 协议族（0 = 自动，4 / 6 = 强制）
    ip_version: u8,
    /// 测试结束后拉取服务端视角的输出（--get-server-output）
    get_server_output: bool,
    /// 按量测试：bytes（-n）/ blocks（-k）优先于时长，二者互斥（引擎语义）
    bytes_to_send: Option<u64>,
    blocks_to_send: Option<u64>,
    /// DSCP 值（0 = 不设置，1-63 映射到 TOS 高 6 位）
    dscp: u32,
    /// TCP 拥塞控制算法（None = 默认；仅 Linux/FreeBSD 生效）
    congestion_algo: Option<String>,
    /// UDP 禁止分片标志（仅 IPv4）
    udp_dont_fragment: bool,
    /// MPTCP 多路径（需两端内核支持）
    mptcp: bool,
}

/// 按量模式推导：按量测试项（tcp-bytes / udp-bytes / tcp-blocks）强制
/// bytes / blocks，其余取全局 transfer_mode。validate 与 client_params_for
/// 共用同一推导，保证「队列里混排按量项 + 常规项」时各项口径一致
fn effective_transfer_mode(request: &TestRequest) -> &str {
    match request.task_id.as_str() {
        "tcp-bytes" | "udp-bytes" => "bytes",
        "tcp-blocks" => "blocks",
        _ => request.transfer_mode.as_str(),
    }
}

fn client_params_for(request: &TestRequest) -> ClientParams {
    // 协议由任务类型推断：udp-* 为 UDP，其余（含 stress）为 TCP，
    // 使 TCP/UDP 测试项可以在同一队列中混合执行
    let protocol = if request.task_id.starts_with("udp-") {
        TransportProtocol::Udp
    } else {
        TransportProtocol::Tcp
    };
    let transfer_mode = effective_transfer_mode(request);
    let mut params = ClientParams {
        protocol,
        duration: request.duration as u32,
        num_streams: 1,
        // TCP/UDP 报文长度分开设置：UDP 取 udp_packet_length（默认 8KB，上限 64KB），
        // TCP 取 packet_length（默认 128KB，上限 1MB）
        blksize: {
            let length = if protocol == TransportProtocol::Udp {
                request.udp_packet_length
            } else {
                request.packet_length as u64
            };
            (length > 0).then_some(length as usize)
        },
        reverse: false,
        bidir: false,
        // 0（界面「不限制」）原样传递，交由引擎按 0 = 不限制处理
        bandwidth_bps: request.bandwidth.saturating_mul(1_000_000),
        interval: request.interval as f64,
        bind_address: (!request.local_ip.trim().is_empty()).then(|| request.local_ip.clone()),
        omit_secs: request.omit_secs,
        window_kb: request.window_kb,
        cport: request.cport,
        ip_version: request.ip_version,
        get_server_output: request.get_server_output,
        // 按量测试：bytes/blocks 与时长互斥（引擎端条件优先级，iperf3 同款）；
        // 按量测试项强制模式，其余取全局参数
        bytes_to_send: (transfer_mode == "bytes")
            .then(|| request.transfer_amount.saturating_mul(1_000_000)),
        blocks_to_send: (transfer_mode == "blocks").then_some(request.transfer_amount),
        dscp: request.dscp,
        // 空字符串 = 不设置；其余原样下发（引擎在非 Linux/FreeBSD 上静默忽略）
        congestion_algo: (!request.congestion_algo.trim().is_empty())
            .then(|| request.congestion_algo.trim().to_string()),
        udp_dont_fragment: request.udp_dont_fragment || request.task_id == "udp-df",
        mptcp: request.mptcp || request.task_id == "tcp-mptcp",
    };
    match request.task_id.as_str() {
        "tcp-parallel" => params.num_streams = request.parallel.max(1) as u32,
        "tcp-reverse" => params.reverse = true,
        "tcp-bidir" => params.bidir = true,
        _ => {}
    }
    params
}

/// 日志文件句柄：回调（同步）与主任务（异步）共用，加锁串行写入
struct TestLog {
    file: Arc<Mutex<StdFile>>,
    working_path: std::path::PathBuf,
    base_name: String,
}
type SessionLog = Arc<TestLog>;

async fn run_engine_client(
    app: AppHandle,
    session_id: String,
    request: TestRequest,
    rx: watch::Receiver<Option<String>>,
    locale_handle: Arc<RwLock<String>>,
) {
    let task_name = task_label(&request.task_id);
    let local_ip = if request.local_ip.trim().is_empty() {
        local_ip_address::local_ip()
            .map(|ip| ip.to_string())
            .unwrap_or_else(|_| "127.0.0.1".into())
    } else {
        request.local_ip.clone()
    };
    let Some(log) = setup_log(&app, &session_id, &request, &local_ip, task_name).await else {
        return;
    };
    // 界面语言运行时可变：日志输出点实时读取，切换语言后立即生效
    let locale = current_locale(&locale_handle);
    let header = tr_format!(
        &locale,
        "引擎: riperf3 {}（纯 Rust 内置，无需安装 iperf3）\n参数: {}, 端口 {}, 时长 {}s, 并发 {}, 带宽 {}, 报文长度 {}, 输出周期 {}s\n\n",
        "Engine: riperf3 {} (pure Rust, built-in, no iperf3 needed)\nParams: {}, port {}, duration {}s, streams {}, bandwidth {}, packet length {}, interval {}s\n\n",
        riperf3::VERSION,
        if client_params_for(&request).protocol == TransportProtocol::Udp { "UDP" } else { "TCP" },
        request.port,
        request.duration,
        client_params_for(&request).num_streams,
        if request.bandwidth > 0 {
            format!("{}M", request.bandwidth)
        } else {
            tr(&locale, "不限制", "unlimited").into()
        },
        if client_params_for(&request).protocol == TransportProtocol::Udp {
            request.udp_packet_length
        } else {
            request.packet_length as u64
        },
        request.interval,
    );
    append_log(&log, &header);
    emit_log(
        &app,
        &session_id,
        &request.task_id,
        "INFO",
        tr_format!(
            &locale,
            "执行：riperf3 -c {}（内嵌引擎）",
            "Running: riperf3 -c {} (embedded engine)",
            request.server_ip
        ),
    );

    let params = client_params_for(&request);
    // iperf3 服务端一次只服务一个测试：队列里上一项刚结束时对端可能尚未回到监听
    // 状态，此刻连上去会收到 ServerBusy。自动重试数次，避免整个测试项被误判为失败。
    let mut attempt: u32 = 0;
    let outcome = loop {
        let builder = engine_client_builder(
            &app,
            &session_id,
            &request,
            &params,
            &log,
            &locale_handle,
            rx.clone(),
        );
        let client = match builder.build() {
            Ok(client) => client,
            Err(error) => {
                fail_engine(
                    &app,
                    &session_id,
                    &request.task_id,
                    &log,
                    &locale,
                    &tr_format!(
                        &locale,
                        "测试配置无效：{}",
                        "Invalid test configuration: {}",
                        error
                    ),
                )
                .await;
                return;
            }
        };
        match client.run().await {
            // 已收到停止信号时不再重试（rx 非 None = 用户已点停止）
            Err(riperf3::RiperfError::ServerBusy)
                if attempt < BUSY_RETRY_MAX && rx.borrow().is_none() =>
            {
                attempt += 1;
                let locale = current_locale(&locale_handle);
                let message = tr_format!(
                    &locale,
                    "服务端正忙，{} 秒后重试（第 {}/{} 次）",
                    "Server busy, retrying in {}s ({}/{})",
                    BUSY_RETRY_DELAY.as_secs(),
                    attempt,
                    BUSY_RETRY_MAX
                );
                append_log(&log, &format!("[WARN] {message}"));
                emit_log(&app, &session_id, &request.task_id, "WARN", message);
                // 等待期间仍要响应「停止测试」，否则用户得干等完整个退避
                let mut wait_rx = rx.clone();
                tokio::select! {
                    _ = sleep(BUSY_RETRY_DELAY) => {}
                    _ = wait_rx.changed() => {}
                }
            }
            other => break other,
        }
    };
    finish_engine(
        &app,
        &session_id,
        &request.task_id,
        &log,
        &request,
        &locale,
        outcome,
    )
    .await;
}

/// 服务端忙时的重试次数与退避间隔（iperf3 服务端串行服务，队列相邻项之间常撞上）
const BUSY_RETRY_MAX: u32 = 3;
const BUSY_RETRY_DELAY: Duration = Duration::from_secs(2);

/// 构造一次 riperf3 客户端 builder。
///
/// 必须每次重试都重新构造：`build()` 会消费 builder，其中的回调闭包也随之被移走。
#[allow(clippy::too_many_arguments)]
fn engine_client_builder(
    app: &AppHandle,
    session_id: &str,
    request: &TestRequest,
    params: &ClientParams,
    log: &SessionLog,
    locale_handle: &Arc<RwLock<String>>,
    rx: watch::Receiver<Option<String>>,
) -> ClientBuilder {
    // 实时指标回调：每输出周期触发一次，发出 metric 事件并写入日志
    let hook_app = app.clone();
    let hook_session = session_id.to_string();
    let hook_task = request.task_id.clone();
    let hook_log = log.clone();
    let hook_locale = locale_handle.clone();
    let on_interval = move |interval: &riperf3::json_report::Interval| {
        if interval.sum.omitted {
            return;
        }
        let sum = &interval.sum;
        let second = sum.end.round() as i64;
        let metric = MetricPoint {
            second,
            bandwidth_mbps: sum.bits_per_second / 1_000_000.0,
            transfer_mb: sum.bytes as f64 / 1_000_000.0,
            jitter_ms: sum.jitter_ms.unwrap_or(0.0),
            loss_percent: sum.lost_percent.unwrap_or(0.0),
            retransmits: sum.retransmits.unwrap_or(0).max(0) as u64,
        };
        emit_metric(&hook_app, &hook_session, &hook_task, metric);
        append_log(
            &hook_log,
            &format!(
                "[INFO] {}",
                format_interval_line(&current_locale(&hook_locale), second, sum)
            ),
        );
    };

    let mut builder = ClientBuilder::new(&request.server_ip)
        .port(Some(request.port))
        .protocol(params.protocol)
        .duration(params.duration)
        .num_streams(params.num_streams)
        .interval(params.interval)
        .json_output(true)
        .emit_output(false)
        .interrupt(rx)
        .on_interval(on_interval)
        .on_connect({
            // 控制连接建立即广播本地端口（客户端「连接状态」展示本次连接的本地端口）
            let app = app.clone();
            let session_id = session_id.to_string();
            let task_id = request.task_id.clone();
            move |addr| {
                let payload = format!(r#"{{"localPort":{}}}"#, addr.port());
                let _ = app.emit(
                    "test-event",
                    TestEvent {
                        session_id: session_id.clone(),
                        task_id: task_id.clone(),
                        event_type: "status".into(),
                        status: None,
                        level: None,
                        message: Some(payload),
                        metric: None,
                        log_path: None,
                    },
                );
            }
        });
    if let Some(size) = params.blksize {
        builder = builder.blksize(size);
    }
    if params.reverse {
        builder = builder.reverse(true);
    }
    if params.bidir {
        builder = builder.bidir(true);
    }
    // 无条件下发（0 = 不限制）：省略调用会让 UDP 落到引擎的 1 Mibit/s 默认值
    builder = builder.bandwidth(params.bandwidth_bps);
    if let Some(addr) = &params.bind_address {
        builder = builder.bind_address(addr);
    }
    // 预热：跳过前 N 秒的统计（排除 TCP 慢启动）；on_interval 回调已跳过
    // omitted 区间，实时图表/日志与最终报告口径一致
    if params.omit_secs > 0 {
        builder = builder.omit(params.omit_secs);
    }
    // 套接字缓冲区（KB → 字节）：0 = 引擎默认（等价 iperf3 的 -w 0 = 自动）。
    // 用 u64 换算防溢出：UI 上限 16384 KB，但请求来自 IPC 边界，不可信
    if params.window_kb > 0 {
        builder = builder.window((params.window_kb as u64 * 1024).min(i32::MAX as u64) as i32);
    }
    // 数据流源端口（--cport）：0 = 不设置，走临时端口
    if params.cport > 0 {
        builder = builder.cport(params.cport);
    }
    // IP 协议族：0 = 自动，仅显式 4 / 6 时下发给引擎（validate 已保证取值合法）
    if params.ip_version == 4 || params.ip_version == 6 {
        builder = builder.ip_version(params.ip_version);
    }
    // 拉取服务端视角输出（--get-server-output）：服务端为文本模式时随结果返回
    if params.get_server_output {
        builder = builder.get_server_output(true);
    }
    // 按量测试（-n / -k）：优先于时长；二者互斥由 transfer_mode 单选保证
    if let Some(bytes) = params.bytes_to_send {
        builder = builder.bytes(bytes);
    }
    if let Some(blocks) = params.blocks_to_send {
        builder = builder.blocks(blocks);
    }
    // DSCP 标记（--dscp）：0 = 不设置；数值字符串由引擎解析并左移 2 位进 TOS 字节
    if params.dscp > 0 {
        builder = builder.dscp(&params.dscp.to_string());
    }
    // 拥塞控制算法（-C）：仅 Linux/FreeBSD 生效，其余平台引擎静默忽略
    if let Some(algo) = &params.congestion_algo {
        builder = builder.congestion(algo);
    }
    // UDP 禁止分片（--dont-fragment）：仅 IPv4，Unix 平台生效
    if params.udp_dont_fragment {
        builder = builder.dont_fragment(true);
    }
    // MPTCP 多路径（-m/--multipath）：需两端内核支持，不支持时连接阶段报错
    if params.mptcp {
        builder = builder.mptcp(true);
    }
    // iperf3 认证：服务端以 --rsa-private-key-path + --authorized-users-path 启动时，
    // 客户端须用服务端公钥加密「用户名+密码」。未启用时前端已把三项清空，这里自然跳过。
    if !request.auth_username.trim().is_empty() {
        builder = builder.username(request.auth_username.trim());
    }
    if !request.auth_password.is_empty() {
        builder = builder.password(&request.auth_password);
    }
    if !request.auth_public_key_path.trim().is_empty() {
        builder = builder.rsa_public_key_path(request.auth_public_key_path.trim());
    }
    // iperf3 3.17 起默认 OAEP 填充，更早的服务端只认 PKCS#1 v1.5
    if request.auth_pkcs1_padding {
        builder = builder.use_pkcs1_padding(true);
    }
    builder
}

// ---------------------------------------------------------------------------
// riperf3 服务端任务：持续监听直到手动停止
// ---------------------------------------------------------------------------

/// 数值防护：NaN / Infinity 无法表示为 JSON 数字，直接格式化会产生非法 JSON
/// （前端 JSON.parse 失败后整条 status 事件被丢弃，服务端概览全部不刷新），统一归零
fn safe_f64(value: f64) -> f64 {
    if value.is_finite() {
        value
    } else {
        0.0
    }
}

/// 服务端最近一个输出周期的统计快照（on_interval 回调写入，心跳任务读取并广播给前端）；
/// 同时记录最近一次连接的客户端地址（服务端概览「对端=客户端」的数据源）
#[derive(Default, Clone)]
struct ServerInterval {
    bandwidth_mbps: f64,
    transfer_mb: f64,
    jitter_ms: f64,
    loss_percent: f64,
    retransmits: u64,
    peer_ip: String,
    peer_port: u16,
}

async fn run_engine_server(
    app: AppHandle,
    session_id: String,
    request: TestRequest,
    rx: watch::Receiver<Option<String>>,
    locale_handle: Arc<RwLock<String>>,
) {
    let task_name = task_label(&request.task_id);
    let local_ip = if request.local_ip.trim().is_empty() {
        local_ip_address::local_ip()
            .map(|ip| ip.to_string())
            .unwrap_or_else(|_| "127.0.0.1".into())
    } else {
        request.local_ip.clone()
    };
    let Some(log) = setup_log(&app, &session_id, &request, &local_ip, task_name).await else {
        return;
    };
    let bind_display = if request.bind_ip.trim().is_empty() {
        "0.0.0.0（所有网卡）".to_string()
    } else {
        request.bind_ip.clone()
    };
    // 界面语言运行时可变：头部日志用启动时语言，循环内每次迭代实时读取
    let locale = current_locale(&locale_handle);
    // 认证启用时在头部日志标注：服务端持有私钥与用户文件，客户端需按「客户端 → 认证」配置
    let auth_display = if request.server_auth_enabled {
        tr_format!(
            &locale,
            "，认证已启用（私钥 {}，用户文件 {}）",
            ", auth enabled (private key {}, users file {})",
            request.server_auth_private_key_path,
            request.server_auth_users_path
        )
    } else {
        String::new()
    };
    // 防护参数标注（0 = 不限制的不显示）：空闲超时 / 单测时长上限 / 带宽上限
    let mut limits: Vec<String> = Vec::new();
    if request.server_idle_timeout > 0 {
        limits.push(tr_format!(
            &locale,
            "空闲超时 {}s",
            "idle timeout {}s",
            request.server_idle_timeout
        ));
    }
    if request.server_max_duration > 0 {
        limits.push(tr_format!(
            &locale,
            "单测上限 {}s",
            "max duration {}s",
            request.server_max_duration
        ));
    }
    if request.server_bitrate_limit_mbps > 0 {
        limits.push(tr_format!(
            &locale,
            "限速 {} Mbps",
            "rate cap {} Mbps",
            request.server_bitrate_limit_mbps
        ));
    }
    let limits_display = if limits.is_empty() {
        String::new()
    } else {
        format!("，{}", limits.join("，"))
    };
    append_log(
        &log,
        &tr_format!(
            locale,
            "引擎: riperf3 {}（纯 Rust 内置，无需安装 iperf3）\n参数: 绑定 {}，监听端口 {}，持续服务{}{}\n\n",
            "Engine: riperf3 {} (pure Rust, built-in, no iperf3 needed)\nParams: bind {}, listen port {}, serving continuously{}{}\n\n",
            riperf3::VERSION,
            bind_display,
            request.port,
            auth_display,
            limits_display
        ),
    );

    // 服务端支持绑定指定 IP（留空 = 绑定所有网卡）；测试进行中由 on_interval 回调标记，
    // 该回调同时采样当前测试的间隔统计（服务端视角的实时曲线数据源）
    let serving = Arc::new(AtomicBool::new(false));
    let latest = Arc::new(Mutex::new(None::<ServerInterval>));
    let mut server_builder = ServerBuilder::new();
    if !request.bind_ip.trim().is_empty() {
        server_builder = server_builder.bind_address(&request.bind_ip);
    }
    // 服务端认证：私钥 + 授权用户文件必须同时提供（对应 iperf3 的
    // --rsa-private-key-path + --authorized-users-path）。凭据解密或校验失败时
    // 引擎按协议直接关闭控制连接（见 riperf3 server.rs 的 authenticate），
    // 客户端表现为「访问被拒」，服务端日志记录具体原因
    if request.server_auth_enabled {
        server_builder = server_builder
            .rsa_private_key_path(request.server_auth_private_key_path.trim())
            .authorized_users_path(request.server_auth_users_path.trim());
        // iperf3 3.17 起默认 OAEP 填充，更早的客户端只认 PKCS#1 v1.5
        if request.server_auth_pkcs1_padding {
            server_builder = server_builder.use_pkcs1_padding(true);
        }
    }
    // 服务端防护参数（0 = 不限制）：空闲超时自动停止 / 拒绝超长测试 / 终止超速测试。
    // 注意 idle_timeout 在 one_off 模式下到期返回 Aborted("idle timeout")，
    // 监听循环据此按「空闲超时停止」退出而非继续监听
    if request.server_idle_timeout > 0 {
        server_builder = server_builder.idle_timeout(request.server_idle_timeout);
    }
    if request.server_max_duration > 0 {
        server_builder = server_builder.server_max_duration(request.server_max_duration);
    }
    if request.server_bitrate_limit_mbps > 0 {
        server_builder = server_builder
            .server_bitrate_limit(request.server_bitrate_limit_mbps.saturating_mul(1_000_000));
    }
    // 服务端统计采样间隔（-i，本地补丁）：与界面「日志输出间隔」一致。
    // 引擎原本固定 1s（无服务端 -i 旋钮），此接线让服务端视角曲线的
    // 采样频率跟随设置
    server_builder = server_builder.interval(request.interval as f64);
    server_builder = server_builder.on_interval({
        let serving = serving.clone();
        let latest = latest.clone();
        move |interval: &riperf3::json_report::Interval| {
            serving.store(true, Ordering::Relaxed);
            if interval.sum.omitted {
                return;
            }
            let sum = &interval.sum;
            // 只更新统计字段，保留对端（客户端）地址
            let mut guard = latest.lock().unwrap();
            let peer = guard
                .as_ref()
                .map(|s| (s.peer_ip.clone(), s.peer_port))
                .unwrap_or_default();
            *guard = Some(ServerInterval {
                bandwidth_mbps: safe_f64(sum.bits_per_second / 1_000_000.0),
                transfer_mb: safe_f64(sum.bytes as f64 / 1_000_000.0),
                jitter_ms: safe_f64(sum.jitter_ms.unwrap_or(0.0)),
                loss_percent: safe_f64(sum.lost_percent.unwrap_or(0.0)),
                retransmits: sum.retransmits.unwrap_or(0).max(0) as u64,
                peer_ip: peer.0,
                peer_port: peer.1,
            });
        }
    });
    // 客户端一建立控制连接就实时广播对端地址（服务端概览「对端=客户端」无需等待测试结束）
    server_builder = server_builder.on_connect({
        let app = app.clone();
        let session_id = session_id.clone();
        let task_id = request.task_id.clone();
        let log = log.clone();
        let locale_handle = locale_handle.clone();
        let latest = latest.clone();
        move |addr| {
            // 界面语言运行时可变：连接日志实时读取当前语言
            let locale = current_locale(&locale_handle);
            let host = addr.ip().to_canonical().to_string();
            let port = addr.port();
            // 记录对端（客户端）地址到快照，后续心跳事件持续携带。
            // 注意：锁必须在 if-let 外统一获取——if-let 的条件临时值（MutexGuard）
            // 存活到整个 if-let 语句结束，在 else 分支里再次 lock 会对同一线程
            // 重复获取 std Mutex 而死锁（客户端首次连接、快照为 None 时必现）
            let mut snapshot_guard = latest.lock().unwrap();
            if let Some(snapshot) = snapshot_guard.as_mut() {
                snapshot.peer_ip = host.clone();
                snapshot.peer_port = port;
            } else {
                *snapshot_guard = Some(ServerInterval {
                    peer_ip: host.clone(),
                    peer_port: port,
                    ..Default::default()
                });
            }
            let connected = tr_format!(
                &locale,
                "客户端 {}:{} 已连接",
                "Client {}:{} connected",
                host,
                port
            );
            append_log(&log, &format!("[INFO] {connected}"));
            emit_log(&app, &session_id, &task_id, "INFO", connected);
            let payload = format!(
                r#"{{"serving":true,"peerIp":"{}","peerPort":{}}}"#,
                host, port
            );
            let _ = app.emit(
                "test-event",
                TestEvent {
                    session_id: session_id.clone(),
                    task_id: task_id.clone(),
                    event_type: "status".into(),
                    status: None,
                    level: None,
                    message: Some(payload),
                    metric: None,
                    log_path: None,
                },
            );
        }
    });
    let server = match server_builder
        .port(Some(request.port))
        .one_off(true)
        .json_output(true)
        .emit_output(false)
        .interrupt(rx)
        .build()
    {
        Ok(server) => server,
        Err(error) => {
            fail_engine(
                &app,
                &session_id,
                &request.task_id,
                &log,
                &locale,
                &tr_format!(
                    &locale,
                    "服务端配置无效：{}",
                    "Invalid server configuration: {}",
                    error
                ),
            )
            .await;
            return;
        }
    };
    let bound = match server.bind().await {
        Ok(bound) => bound,
        Err(error) => {
            fail_engine(
                &app,
                &session_id,
                &request.task_id,
                &log,
                &locale,
                &tr_format!(
                    &locale,
                    "无法监听端口 {}：{}",
                    "Cannot listen on port {}: {}",
                    request.port,
                    error
                ),
            )
            .await;
            return;
        }
    };
    append_log(
        &log,
        &format!(
            "[INFO] {}",
            tr_format!(
                locale,
                "服务端已就绪，监听 {}:{}",
                "Server ready, listening on {}:{}",
                bind_display,
                request.port
            )
        ),
    );
    emit_log(
        &app,
        &session_id,
        &request.task_id,
        "INFO",
        tr_format!(
            locale,
            "服务端已就绪，监听 {}:{}",
            "Server ready, listening on {}:{}",
            bind_display,
            request.port
        ),
    );

    // 周期状态输出：按「日志输出间隔」每隔 N 秒写一次运行日志与统计信息（并发于监听循环）
    let interval_secs = request.interval.max(1);
    let completed = Arc::new(AtomicU64::new(0));
    let bind_short = if request.bind_ip.trim().is_empty() {
        "0.0.0.0".to_string()
    } else {
        request.bind_ip.clone()
    };
    let port = request.port;
    let heartbeat = {
        let log = log.clone();
        let app = app.clone();
        let session_id = session_id.clone();
        let task_id = request.task_id.clone();
        let completed = completed.clone();
        let serving = serving.clone();
        let latest = latest.clone();
        let locale_handle = locale_handle.clone();
        tauri::async_runtime::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(interval_secs));
            ticker.tick().await; // 跳过立即触发的第一次
            let started = std::time::Instant::now();
            loop {
                ticker.tick().await;
                // 界面语言运行时可变：心跳日志与状态事件实时读取当前语言
                let locale = current_locale(&locale_handle);
                let uptime = started.elapsed().as_secs();
                let is_serving = serving.load(Ordering::Relaxed);
                let done = completed.load(Ordering::Relaxed);
                let snapshot = latest.lock().unwrap().clone();
                // 文本日志行（写文件 + 广播）
                let status_text = if is_serving {
                    tr(&locale, "测试进行中", "test in progress")
                } else {
                    tr(&locale, "空闲", "idle")
                };
                let message = tr_format!(
                    &locale,
                    "运行状态：监听 {}:{}，已运行 {}s，累计完成 {} 次测试，当前{}",
                    "Status: listening on {}:{}, up {}s, {} tests completed, currently {}",
                    bind_short,
                    port,
                    uptime,
                    done,
                    status_text
                );
                append_log(&log, &format!("[INFO] {message}"));
                emit_log(&app, &session_id, &task_id, "INFO", message);
                // 结构化统计事件：供服务端窗口的概览与实时曲线使用（携带当前测试的间隔统计与最近客户端地址）
                let stats = match &snapshot {
                    Some(s) => format!(
                        r#","bandwidthMbps":{},"transferMb":{},"jitterMs":{},"lossPercent":{},"retransmits":{}"#,
                        safe_f64(s.bandwidth_mbps),
                        safe_f64(s.transfer_mb),
                        safe_f64(s.jitter_ms),
                        safe_f64(s.loss_percent),
                        s.retransmits
                    ),
                    None => String::new(),
                };
                let peer = match &snapshot {
                    Some(s) if !s.peer_ip.is_empty() => {
                        format!(r#","peerIp":"{}","peerPort":{}"#, s.peer_ip, s.peer_port)
                    }
                    _ => String::new(),
                };
                let payload = format!(
                    r#"{{"uptime":{},"completed":{},"serving":{}{}{}}}"#,
                    uptime, done, is_serving, stats, peer
                );
                let _ = app.emit(
                    "test-event",
                    TestEvent {
                        session_id: session_id.clone(),
                        task_id: task_id.clone(),
                        event_type: "status".into(),
                        status: None,
                        level: None,
                        message: Some(payload),
                        metric: None,
                        log_path: None,
                    },
                );
            }
        })
    };

    loop {
        // 每次迭代实时读取界面语言（切换语言后服务端日志立即跟随）
        let locale = current_locale(&locale_handle);
        match bound.run_once().await {
            Ok(outcome) => {
                serving.store(false, Ordering::Relaxed);
                completed.fetch_add(1, Ordering::Relaxed);
                // 记录本次连接的客户端地址（服务端概览「对端=客户端」的数据源）
                let peer = outcome
                    .report
                    .start
                    .connected
                    .first()
                    .map(|c| (c.remote_host.clone(), c.remote_port));
                // 测试结束（客户端已断开）：清空对端地址与统计，心跳事件不再携带
                *latest.lock().unwrap() = None;
                if outcome.termination == Termination::Interrupted {
                    heartbeat.abort();
                    append_log(
                        &log,
                        &format!(
                            "\n{}",
                            tr(
                                &locale,
                                "测试结果: 服务端已停止（手动停止）",
                                "Result: server stopped (manual stop)"
                            )
                        ),
                    );
                    emit_log(
                        &app,
                        &session_id,
                        &request.task_id,
                        "INFO",
                        tr(
                            &locale,
                            "服务端已停止（手动停止）",
                            "Server stopped (manual stop)",
                        )
                        .into(),
                    );
                    finish_ok(&app, &session_id, &request.task_id, &log, "stopped").await;
                    return;
                }
                // 单次测试结束：写入汇总并继续监听
                let peer_text = peer
                    .as_ref()
                    .map(|(ip, p)| format!("{ip}:{p}"))
                    .unwrap_or_else(|| tr(&locale, "未知地址", "unknown address").to_string());
                append_log(
                    &log,
                    &format!(
                        "\n[INFO] {}",
                        tr_format!(
                            locale,
                            "客户端 {} 完成测试，汇总如下：",
                            "Client {} finished a test, summary:",
                            peer_text
                        )
                    ),
                );
                append_engine_summary(&log, &outcome.report, &locale);
                emit_log(
                    &app,
                    &session_id,
                    &request.task_id,
                    "INFO",
                    tr_format!(
                        locale,
                        "一次测试完成（客户端 {}），继续监听…",
                        "Test finished (client {}), still listening…",
                        peer_text
                    ),
                );
            }
            Err(error) => {
                serving.store(false, Ordering::Relaxed);
                *latest.lock().unwrap() = None;
                // 空闲时收到停止信号返回 Aborted，正常退出；idle_timeout 到期
                // 同样以 Aborted("idle timeout") 返回（one_off 下退出而非重启），
                // 按原因区分日志文案
                if let riperf3::RiperfError::Aborted(msg) = &error {
                    heartbeat.abort();
                    let (result_zh, result_en) = if msg == "idle timeout" {
                        (
                            "测试结果: 服务端已停止（空闲超时）",
                            "Result: server stopped (idle timeout)",
                        )
                    } else {
                        (
                            "测试结果: 服务端已停止（手动停止）",
                            "Result: server stopped (manual stop)",
                        )
                    };
                    let (reason_zh, reason_en) = if msg == "idle timeout" {
                        ("服务端已停止（空闲超时）", "Server stopped (idle timeout)")
                    } else {
                        ("服务端已停止（手动停止）", "Server stopped (manual stop)")
                    };
                    append_log(&log, &format!("\n{}", tr(&locale, result_zh, result_en)));
                    emit_log(
                        &app,
                        &session_id,
                        &request.task_id,
                        "INFO",
                        tr(&locale, reason_zh, reason_en).into(),
                    );
                    finish_ok(&app, &session_id, &request.task_id, &log, "stopped").await;
                    return;
                }
                let warn = tr_format!(
                    locale,
                    "一次连接处理失败：{}，继续监听",
                    "Failed to handle a connection: {}, still listening",
                    error
                );
                append_log(&log, &format!("[WARN] {warn}"));
                emit_log(&app, &session_id, &request.task_id, "WARN", warn);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ping 任务（保留系统进程调用，riperf3 不支持 ICMP）
// ---------------------------------------------------------------------------

async fn run_ping(
    app: AppHandle,
    session_id: String,
    request: TestRequest,
    cancelled: Arc<AtomicBool>,
    child_pids: Arc<AsyncMutex<HashSet<u32>>>,
    locale_handle: Arc<RwLock<String>>,
) {
    let task_name = task_label(&request.task_id);
    // 界面语言运行时可变：ping 日志实时读取当前语言
    let locale = current_locale(&locale_handle);
    let local_ip = if request.local_ip.trim().is_empty() {
        local_ip_address::local_ip()
            .map(|ip| ip.to_string())
            .unwrap_or_else(|_| "127.0.0.1".into())
    } else {
        request.local_ip.clone()
    };
    let Some(log) = setup_log(&app, &session_id, &request, &local_ip, task_name).await else {
        return;
    };
    let args = if cfg!(windows) {
        vec!["-n".into(), "4".into(), request.server_ip.clone()]
    } else {
        vec!["-c".into(), "4".into(), request.server_ip.clone()]
    };
    append_log(&log, &format!("[INFO] 执行：ping {}", args.join(" ")));
    emit_log(
        &app,
        &session_id,
        &request.task_id,
        "INFO",
        tr_format!(locale, "执行：ping {}", "Running: ping {}", args.join(" ")),
    );

    let mut command = Command::new("ping");
    command
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let message = tr_format!(
                locale,
                "启动 ping 失败：{}",
                "Failed to start ping: {}",
                error
            );
            append_log(&log, &format!("[ERROR] {message}"));
            fail_engine(&app, &session_id, &request.task_id, &log, &locale, &message).await;
            return;
        }
    };
    let pid = child.id();
    if let Some(pid) = pid {
        child_pids.lock().await.insert(pid);
    }
    let (tx, mut rx) = mpsc::channel::<(String, bool)>(128);
    if let Some(stdout) = child.stdout.take() {
        let tx = tx.clone();
        tokio::spawn(read_lines(stdout, tx, false));
    }
    if let Some(stderr) = child.stderr.take() {
        let tx = tx.clone();
        tokio::spawn(read_lines(stderr, tx, true));
    }
    drop(tx);
    let mut final_success = false;
    let mut final_stopped = false;
    loop {
        tokio::select! {
            status = child.wait() => {
                final_success = status.map(|value| value.success()).unwrap_or(false);
                break;
            }
            line = rx.recv() => {
                if let Some((line, is_error)) = line {
                    let level = if is_error { "WARN" } else { "INFO" };
                    append_log(&log, &format!("[{level}] {line}"));
                    emit_log(&app, &session_id, &request.task_id, level, line.clone());
                    if let Some(metric) = parse_ping_metric(&line) {
                        emit_metric(&app, &session_id, &request.task_id, metric);
                    }
                }
            }
            _ = sleep(Duration::from_millis(100)) => {
                if cancelled.load(Ordering::SeqCst) {
                    final_stopped = true;
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    break;
                }
            }
        }
    }
    if let Some(pid) = pid {
        child_pids.lock().await.remove(&pid);
    }
    append_log(
        &log,
        &format!(
            "\n{}: {}",
            tr(&locale, "测试结果", "Result"),
            if final_success {
                tr(&locale, "完成", "completed")
            } else {
                tr(&locale, "未完成", "incomplete")
            }
        ),
    );
    if final_stopped {
        finish_ok(&app, &session_id, &request.task_id, &log, "stopped").await;
    } else if final_success {
        finish_ok(&app, &session_id, &request.task_id, &log, "success").await;
    } else {
        let message = tr(
            &locale,
            "ping 测试进程异常退出",
            "Ping process exited unexpectedly",
        )
        .to_string();
        fail_engine(&app, &session_id, &request.task_id, &log, &locale, &message).await;
    }
}

async fn read_lines<R: tokio::io::AsyncRead + Unpin>(
    reader: R,
    tx: mpsc::Sender<(String, bool)>,
    is_error: bool,
) {
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if tx.send((line, is_error)).await.is_err() {
            break;
        }
    }
}

fn parse_ping_metric(line: &str) -> Option<MetricPoint> {
    let ping = Regex::new(r"(?i)(?:time|时间)[=<＝]\s*(\d+(?:\.\d+)?)\s*ms").ok()?;
    ping.captures(line).map(|c| MetricPoint {
        jitter_ms: c[1].parse().unwrap_or(0.0),
        ..Default::default()
    })
}

// ---------------------------------------------------------------------------
// 日志与结果处理
// ---------------------------------------------------------------------------

/// 创建测试日志文件并写入文件头，失败时发出错误事件
async fn setup_log(
    app: &AppHandle,
    session_id: &str,
    request: &TestRequest,
    local_ip: &str,
    task_name: &str,
) -> Option<SessionLog> {
    let stamp = Local::now().format("%Y%m%d%H%M%S").to_string();
    // 服务端与客户端日志分开记录：Server-{本机IP}-{端口}-{时间} / Client-{本机IP}-{对端IP}-{测试项}-{时间}
    let base_name = if request.mode == "server" || request.task_id == "server" {
        format!("Server-{}-{}-{}", safe_name(local_ip), request.port, stamp)
    } else {
        format!(
            "Client-{}-{}-{}-{}",
            safe_name(local_ip),
            safe_name(&request.server_ip),
            safe_name(task_name),
            stamp
        )
    };
    let log_dir = app
        .path()
        .app_log_dir()
        .unwrap_or_else(|_| std::env::temp_dir().join("linkgauge"))
        .join("tests");
    if let Err(error) = fs::create_dir_all(&log_dir).await {
        emit_error(
            app,
            session_id,
            &request.task_id,
            tr_format!(
                &request.locale,
                "无法创建日志目录：{}",
                "Cannot create log directory: {}",
                error
            ),
            None,
        );
        return None;
    }
    let working_path = log_dir.join(format!("{base_name}-进行中.log"));
    let file = match OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&working_path)
        .await
    {
        Ok(file) => file,
        Err(error) => {
            emit_error(
                app,
                session_id,
                &request.task_id,
                tr_format!(
                    &request.locale,
                    "无法创建日志文件：{}",
                    "Cannot create log file: {}",
                    error
                ),
                None,
            );
            return None;
        }
    };
    let header = tr_format!(
        &request.locale,
        "测试时间: {}\n客户端IP: {}\n服务端IP: {}\n测试模式: {}\n测试项目: {}\n",
        "Test time: {}\nClient IP: {}\nServer IP: {}\nMode: {}\nTest item: {}\n",
        Local::now().format("%Y-%m-%d %H:%M:%S"),
        local_ip,
        request.server_ip,
        request.mode,
        task_name
    );
    let log = Arc::new(TestLog {
        file: Arc::new(Mutex::new(file.into_std().await)),
        working_path,
        base_name,
    });
    append_log(&log, &header);
    Some(log)
}

/// 同步追加一行日志（实时回调与主任务共用）
fn append_log(log: &SessionLog, line: &str) {
    if let Ok(mut file) = log.file.lock() {
        let _ = file.write_all(line.as_bytes());
    }
}

/// 客户端结束时按 RunOutcome 归类结果：成功 / 手动停止 / 失败
async fn finish_engine(
    app: &AppHandle,
    session_id: &str,
    task_id: &str,
    log: &SessionLog,
    request: &TestRequest,
    locale: &str,
    outcome: Result<riperf3::RunOutcome, riperf3::RiperfError>,
) {
    match outcome {
        Err(error) => {
            // 两类可操作的失败给出具体排查方向，其余沿用引擎原文
            let message = match &error {
                riperf3::RiperfError::ServerBusy => tr_format!(
                    locale,
                    "服务端忙：iperf3 一次只服务一个测试，重试 {} 次后仍被拒绝。请确认没有其他客户端正在占用该服务端。",
                    "Server busy: iperf3 serves one test at a time and still refused after {} retries. Check that no other client is using that server.",
                    BUSY_RETRY_MAX
                ),
                riperf3::RiperfError::AccessDenied => tr(
                    locale,
                    "认证被服务端拒绝：请核对用户名、密码，以及 RSA 公钥是否与服务端 --authorized-users-path 中的条目匹配；若对端 iperf3 低于 3.17，需勾选「PKCS#1 填充」。",
                    "Authentication rejected: verify the username, password, and that the RSA public key matches an entry in the server's --authorized-users-path. Tick \"PKCS#1 padding\" if the peer iperf3 is older than 3.17.",
                )
                .to_string(),
                _ => tr_format!(locale, "测试失败：{}", "Test failed: {}", error),
            };
            fail_engine(app, session_id, task_id, log, locale, &message).await;
        }
        Ok(outcome) => {
            let report = &outcome.report;
            append_log(
                log,
                &format!(
                    "\n[INFO] {}",
                    tr(
                        locale,
                        "测试结束，最终汇总：",
                        "Test finished, final summary:"
                    )
                ),
            );
            append_engine_summary(log, report, locale);
            // --get-server-output：服务端视角的汇总文本（标准 iperf3 服务端为
            // 文本模式时才会产生；本机 LinkGauge 服务端是 JSON 模式，通常为空）。
            // 写入测试日志并广播一条日志事件，随日志一并进入报告
            if let Some(text) = report
                .server_output_text
                .as_deref()
                .filter(|t| !t.trim().is_empty())
            {
                let header = tr(
                    locale,
                    "服务端输出（--get-server-output）：",
                    "Server output (--get-server-output):",
                );
                let block = format!("\n[INFO] {header}\n{text}");
                append_log(log, &block);
                emit_log(app, session_id, task_id, "INFO", block);
            }
            // 汇总终点即实测结束秒（按量模式 / 预热下与名义 duration 不同，须取实测值）
            let measured_end = report
                .end
                .sum_sent
                .as_ref()
                .or(report.end.sum_received.as_ref())
                .map(|s| s.end.round() as i64)
                .unwrap_or(request.duration as i64);
            emit_final_metric(app, session_id, task_id, report, measured_end);
            match outcome.termination {
                Termination::Completed => {
                    append_log(
                        log,
                        &format!("\n{}", tr(locale, "测试结果: 完成", "Result: completed")),
                    );
                    finish_ok(app, session_id, task_id, log, "success").await;
                }
                Termination::Interrupted => {
                    append_log(
                        log,
                        &format!(
                            "\n{}",
                            tr(locale, "测试结果: 手动停止", "Result: manual stop")
                        ),
                    );
                    finish_ok(app, session_id, task_id, log, "stopped").await;
                }
                Termination::ServerTerminated => {
                    let message = tr(
                        locale,
                        "服务端主动终止了测试",
                        "The server terminated the test",
                    )
                    .to_string();
                    fail_engine(app, session_id, task_id, log, locale, &message).await;
                }
                Termination::ServerError(msg) => {
                    let message = tr_format!(
                        locale,
                        "服务端返回错误：{}",
                        "Server returned an error: {}",
                        msg
                    );
                    fail_engine(app, session_id, task_id, log, locale, &message).await;
                }
                other => {
                    let message = tr_format!(
                        locale,
                        "测试异常结束：{:?}",
                        "Test ended abnormally: {:?}",
                        other
                    );
                    fail_engine(app, session_id, task_id, log, locale, &message).await;
                }
            }
        }
    }
}

/// 追加最终汇总（发送/接收方向的传输量与平均带宽，UDP 附抖动丢包）
fn append_engine_summary(log: &SessionLog, report: &riperf3::Report, locale: &str) {
    if let Some(sent) = &report.end.sum_sent {
        let mut line = tr_format!(
            locale,
            "发送方向: {:.2} MBytes, 平均 {:.2} Mbits/sec",
            "Sent: {:.2} MBytes, average {:.2} Mbits/sec",
            sent.bytes as f64 / 1_000_000.0,
            sent.bits_per_second / 1_000_000.0
        );
        if let Some(retransmits) = sent.retransmits {
            line.push_str(&tr_format!(
                locale,
                ", 重传 {}",
                ", retransmits {}",
                retransmits
            ));
        }
        append_log(log, &format!("[INFO] {line}"));
    }
    if let Some(received) = &report.end.sum_received {
        let mut line = tr_format!(
            locale,
            "接收方向: {:.2} MBytes, 平均 {:.2} Mbits/sec",
            "Received: {:.2} MBytes, average {:.2} Mbits/sec",
            received.bytes as f64 / 1_000_000.0,
            received.bits_per_second / 1_000_000.0
        );
        if let Some(jitter) = received.jitter_ms {
            line.push_str(&tr_format!(
                locale,
                ", 抖动 {:.3} ms, 丢包 {}/{} ({:.2}%)",
                ", jitter {:.3} ms, loss {}/{} ({:.2}%)",
                jitter,
                received.lost_packets.unwrap_or(0),
                received.packets.unwrap_or(0),
                received.lost_percent.unwrap_or(0.0)
            ));
        }
        append_log(log, &format!("[INFO] {line}"));
    }
}

/// 补发一个最终指标点：携带对端方向的抖动/丢包与全程平均带宽，供前端汇总统计
fn emit_final_metric(
    app: &AppHandle,
    session_id: &str,
    task_id: &str,
    report: &riperf3::Report,
    second: i64,
) {
    // 取传输量较大的方向作为统计口径（正向 / 反向 / 双向通用）
    let side = match (
        report.end.sum_sent.as_ref(),
        report.end.sum_received.as_ref(),
    ) {
        (Some(sent), Some(received)) => Some(if sent.bytes >= received.bytes {
            sent
        } else {
            received
        }),
        (Some(sent), None) => Some(sent),
        (None, Some(received)) => Some(received),
        (None, None) => None,
    };
    if let Some(side) = side {
        emit_metric(
            app,
            session_id,
            task_id,
            MetricPoint {
                second,
                bandwidth_mbps: side.bits_per_second / 1_000_000.0,
                transfer_mb: side.bytes as f64 / 1_000_000.0,
                jitter_ms: side.jitter_ms.unwrap_or(0.0),
                loss_percent: side.lost_percent.unwrap_or(0.0),
                retransmits: side.retransmits.unwrap_or(0).max(0) as u64,
            },
        );
    }
}

/// 逐秒指标行的日志格式（近似 iperf3 文本输出）
fn format_interval_line(
    locale: &str,
    second: i64,
    sum: &riperf3::json_report::IntervalSum,
) -> String {
    let mut line = tr_format!(
        locale,
        "第 {} 秒: {:.2} MBytes, {:.2} Mbits/sec",
        "Second {}: {:.2} MBytes, {:.2} Mbits/sec",
        second,
        sum.bytes as f64 / 1_000_000.0,
        sum.bits_per_second / 1_000_000.0
    );
    if let Some(jitter) = sum.jitter_ms {
        line.push_str(&tr_format!(
            locale,
            ", 抖动 {:.3} ms, 丢包 {}/{} ({:.2}%)",
            ", jitter {:.3} ms, loss {}/{} ({:.2}%)",
            jitter,
            sum.lost_packets.unwrap_or(0),
            sum.packets.unwrap_or(0),
            sum.lost_percent.unwrap_or(0.0)
        ));
    }
    if let Some(retransmits) = sum.retransmits {
        line.push_str(&tr_format!(
            locale,
            ", 重传 {}",
            ", retransmits {}",
            retransmits
        ));
    }
    line
}

async fn finish_ok(
    app: &AppHandle,
    session_id: &str,
    task_id: &str,
    log: &SessionLog,
    status: &str,
) {
    let success = status == "success";
    let log_path = finish_log(log, success).await;
    emit_complete(app, session_id, task_id, status, log_path);
}

async fn fail_engine(
    app: &AppHandle,
    session_id: &str,
    task_id: &str,
    log: &SessionLog,
    locale: &str,
    message: &str,
) {
    append_log(log, &format!("[ERROR] {message}"));
    let log_path = finish_log(log, false).await;
    let full = format!(
        "{message}\n{}",
        tr_format!(locale, "详细日志：{}", "Detailed log: {}", log_path)
    );
    emit_error(app, session_id, task_id, full, Some(log_path));
}

/// 关闭日志文件并按完成状态重命名（完成/未完成/手动停止均保留日志）
async fn finish_log(log: &SessionLog, success: bool) -> String {
    if let Ok(mut file) = log.file.lock() {
        let _ = file.flush();
    }
    let final_path = log.working_path.with_file_name(format!(
        "{}-{}.log",
        log.base_name,
        if success { "completed" } else { "incomplete" }
    ));
    let _ = fs::rename(&log.working_path, &final_path).await;
    final_path.to_string_lossy().to_string()
}

fn safe_name(value: &str) -> String {
    value
        .chars()
        .map(|c| if "<>:\"/\\|?*".contains(c) { '_' } else { c })
        .collect()
}

fn task_label(id: &str) -> &str {
    match id {
        "ping" => "Ping连通性测试",
        "tcp-single" => "TCP单向带宽",
        "tcp-bidir" => "TCP双向带宽",
        "tcp-parallel" => "TCP多并发流",
        "udp-bandwidth" => "UDP带宽",
        "udp-loss" => "UDP抖动丢包",
        "tcp-reverse" => "TCP反向测试",
        "stress" => "持续压力测试",
        "server" => "riperf3服务端",
        _ => "网络测试",
    }
}

fn emit_log(app: &AppHandle, session: &str, task: &str, level: &str, message: String) {
    let _ = app.emit(
        "test-event",
        TestEvent {
            session_id: session.into(),
            task_id: task.into(),
            event_type: "log".into(),
            status: None,
            level: Some(level.into()),
            message: Some(message),
            metric: None,
            log_path: None,
        },
    );
}
fn emit_metric(app: &AppHandle, session: &str, task: &str, metric: MetricPoint) {
    let _ = app.emit(
        "test-event",
        TestEvent {
            session_id: session.into(),
            task_id: task.into(),
            event_type: "metric".into(),
            status: None,
            level: None,
            message: None,
            metric: Some(metric),
            log_path: None,
        },
    );
}
fn emit_complete(app: &AppHandle, session: &str, task: &str, status: &str, path: String) {
    let _ = app.emit(
        "test-event",
        TestEvent {
            session_id: session.into(),
            task_id: task.into(),
            event_type: "complete".into(),
            status: Some(status.into()),
            level: None,
            message: None,
            metric: None,
            log_path: Some(path),
        },
    );
}
fn emit_error(app: &AppHandle, session: &str, task: &str, message: String, path: Option<String>) {
    let _ = app.emit(
        "test-event",
        TestEvent {
            session_id: session.into(),
            task_id: task.into(),
            event_type: "error".into(),
            status: Some("failed".into()),
            level: Some("ERROR".into()),
            message: Some(message),
            metric: None,
            log_path: path,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::{client_params_for, validate};
    use crate::models::TestRequest;
    use riperf3::TransportProtocol;

    fn request(task_id: &str, mode: &str, protocol: &str) -> TestRequest {
        TestRequest {
            task_id: task_id.into(),
            mode: mode.into(),
            protocol: protocol.into(),
            server_ip: "192.168.1.100".into(),
            local_ip: "192.168.1.50".into(),
            bind_ip: String::new(),
            locale: String::new(),
            port: 5201,
            duration: 10,
            parallel: 4,
            bandwidth: 0,
            packet_length: 1024,
            udp_packet_length: 8192,
            interval: 1,
            omit_secs: 0,
            window_kb: 0,
            cport: 0,
            ip_version: 0,
            get_server_output: false,
            transfer_mode: "time".into(),
            transfer_amount: 0,
            dscp: 0,
            congestion_algo: String::new(),
            udp_dont_fragment: false,
            mptcp: false,
            auth_username: String::new(),
            auth_password: String::new(),
            auth_public_key_path: String::new(),
            auth_pkcs1_padding: false,
            server_auth_enabled: false,
            server_auth_private_key_path: String::new(),
            server_auth_users_path: String::new(),
            server_auth_pkcs1_padding: false,
            server_idle_timeout: 0,
            server_max_duration: 0,
            server_bitrate_limit_mbps: 0,
        }
    }

    #[test]
    fn tcp_single_maps_to_basic_client() {
        let params = client_params_for(&request("tcp-single", "client", "tcp"));
        assert_eq!(params.protocol, TransportProtocol::Tcp);
        assert_eq!(params.duration, 10);
        assert_eq!(params.num_streams, 1);
        assert!(!params.reverse);
        assert!(!params.bidir);
        assert_eq!(params.blksize, Some(1024));
        assert_eq!(params.interval, 1.0);
        assert_eq!(params.bind_address.as_deref(), Some("192.168.1.50"));
    }

    #[test]
    fn tcp_parallel_maps_num_streams() {
        let params = client_params_for(&request("tcp-parallel", "client", "tcp"));
        assert_eq!(params.num_streams, 4);
    }

    #[test]
    fn tcp_bidir_and_reverse_flags() {
        assert!(client_params_for(&request("tcp-bidir", "client", "tcp")).bidir);
        assert!(client_params_for(&request("tcp-reverse", "client", "tcp")).reverse);
    }

    #[test]
    fn udp_maps_protocol() {
        assert_eq!(
            client_params_for(&request("udp-bandwidth", "client", "udp")).protocol,
            TransportProtocol::Udp
        );
        assert_eq!(
            client_params_for(&request("udp-loss", "client", "udp")).protocol,
            TransportProtocol::Udp
        );
    }

    #[test]
    fn tcp_tasks_map_to_tcp_regardless_of_protocol_field() {
        // 混合队列中协议由 task_id 决定，request.protocol 字段不再参与
        assert_eq!(
            client_params_for(&request("tcp-single", "client", "udp")).protocol,
            TransportProtocol::Tcp
        );
        assert_eq!(
            client_params_for(&request("stress", "client", "tcp")).protocol,
            TransportProtocol::Tcp
        );
    }

    #[test]
    fn bandwidth_zero_means_unlimited() {
        assert_eq!(
            client_params_for(&request("tcp-single", "client", "tcp")).bandwidth_bps,
            0
        );
    }

    #[test]
    fn bandwidth_mbps_to_bps() {
        let mut req = request("tcp-single", "client", "tcp");
        req.bandwidth = 100;
        assert_eq!(client_params_for(&req).bandwidth_bps, 100_000_000);
    }

    /// 回归：界面选「不限制」的 UDP 测试必须解析出 0 并显式下发。
    /// 曾经映射为 None 且调用点「为 None 就不调用 bandwidth()」，
    /// 于是 riperf3 套用 iperf3 的 UDP_RATE 默认值，把不限速的 UDP
    /// 测试实际限到约 1 Mbps。
    #[test]
    fn udp_unlimited_bandwidth_stays_zero() {
        for task in ["udp-bandwidth", "udp-loss"] {
            let mut req = request(task, "client", "udp");
            req.bandwidth = 0;
            assert_eq!(
                client_params_for(&req).bandwidth_bps,
                0,
                "{task}：不限制必须解析为 0，否则引擎会套用 1 Mibit/s 默认值"
            );
        }
    }

    #[test]
    fn packet_length_zero_omits_blksize() {
        let mut req = request("tcp-single", "client", "tcp");
        req.packet_length = 0;
        assert_eq!(client_params_for(&req).blksize, None);
    }

    #[test]
    fn udp_uses_udp_packet_length() {
        let mut req = request("udp-bandwidth", "client", "udp");
        req.packet_length = 131072;
        req.udp_packet_length = 8192;
        assert_eq!(client_params_for(&req).blksize, Some(8192));
    }

    #[test]
    fn tcp_uses_packet_length() {
        let mut req = request("tcp-single", "client", "tcp");
        req.packet_length = 131072;
        req.udp_packet_length = 8192;
        assert_eq!(client_params_for(&req).blksize, Some(131072));
    }

    #[test]
    fn server_auth_requires_both_files() {
        let mut req = request("server", "server", "tcp");
        req.duration = 0;
        req.server_auth_enabled = true;
        // 只填一个路径：必须拒绝（引擎在两者均提供时才校验凭据）
        req.server_auth_private_key_path = "key.pem".into();
        assert!(validate(&req).is_err());
        req.server_auth_users_path = "users.csv".into();
        assert!(validate(&req).is_ok());
        // 未启用时缺路径不报错（保持旧行为）
        req.server_auth_enabled = false;
        req.server_auth_private_key_path.clear();
        req.server_auth_users_path.clear();
        assert!(validate(&req).is_ok());
        // 客户端模式不受服务端认证字段影响
        let mut client = request("tcp-single", "client", "tcp");
        client.server_auth_enabled = true;
        assert!(validate(&client).is_ok());
    }

    #[test]
    fn omit_and_window_map_through_params() {
        let mut req = request("tcp-single", "client", "tcp");
        req.omit_secs = 5;
        req.window_kb = 256;
        let params = client_params_for(&req);
        assert_eq!(params.omit_secs, 5);
        assert_eq!(params.window_kb, 256);
        // 默认 0 = 不预热 / 自动缓冲
        let params = client_params_for(&request("tcp-single", "client", "tcp"));
        assert_eq!(params.omit_secs, 0);
        assert_eq!(params.window_kb, 0);
    }

    #[test]
    fn omit_must_be_shorter_than_duration() {
        let mut req = request("tcp-single", "client", "tcp");
        req.duration = 10;
        // 预热 == 时长：拒绝（统计区间为空）
        req.omit_secs = 10;
        assert!(validate(&req).is_err());
        // 预热 > 时长：拒绝
        req.omit_secs = 11;
        assert!(validate(&req).is_err());
        // 预热 < 时长：通过
        req.omit_secs = 9;
        assert!(validate(&req).is_ok());
        // 0 = 不预热：通过
        req.omit_secs = 0;
        assert!(validate(&req).is_ok());
    }

    #[test]
    fn window_kb_capped_at_16mb() {
        let mut req = request("tcp-single", "client", "tcp");
        req.window_kb = 16384;
        assert!(validate(&req).is_ok());
        req.window_kb = 16385;
        assert!(validate(&req).is_err());
    }

    #[test]
    fn cport_and_ip_version_map_through_params() {
        let mut req = request("tcp-single", "client", "tcp");
        req.cport = 40000;
        req.ip_version = 6;
        let params = client_params_for(&req);
        assert_eq!(params.cport, 40000);
        assert_eq!(params.ip_version, 6);
        // 默认 0 = 自动
        let params = client_params_for(&request("tcp-single", "client", "tcp"));
        assert_eq!(params.cport, 0);
        assert_eq!(params.ip_version, 0);
    }

    #[test]
    fn ip_version_rejects_invalid_values() {
        let mut req = request("tcp-single", "client", "tcp");
        for valid in [0, 4, 6] {
            req.ip_version = valid;
            assert!(validate(&req).is_ok(), "ip_version={valid} 应通过");
        }
        for invalid in [1, 3, 5, 7, 255] {
            req.ip_version = invalid;
            assert!(validate(&req).is_err(), "ip_version={invalid} 应被拒绝");
        }
    }

    #[test]
    fn server_protection_params_validated() {
        let mut req = request("server", "server", "tcp");
        req.duration = 0;
        // 默认 0 = 不限制：全部通过
        assert!(validate(&req).is_ok());
        // 上限边界：86400 通过，超限拒绝
        req.server_idle_timeout = 86400;
        req.server_max_duration = 86400;
        assert!(validate(&req).is_ok());
        req.server_idle_timeout = 86401;
        assert!(validate(&req).is_err());
        req.server_idle_timeout = 0;
        req.server_max_duration = 86401;
        assert!(validate(&req).is_err());
        req.server_max_duration = 0;
        // 带宽上限：1_000_000 Mbps 通过，超限拒绝
        req.server_bitrate_limit_mbps = 1_000_000;
        assert!(validate(&req).is_ok());
        req.server_bitrate_limit_mbps = 1_000_001;
        assert!(validate(&req).is_err());
    }

    #[test]
    fn transfer_mode_maps_bytes_and_blocks() {
        let mut req = request("tcp-single", "client", "tcp");
        // bytes：MB → 字节（十进制）
        req.transfer_mode = "bytes".into();
        req.transfer_amount = 10;
        let params = client_params_for(&req);
        assert_eq!(params.bytes_to_send, Some(10_000_000));
        assert_eq!(params.blocks_to_send, None);
        // blocks：原样块数
        req.transfer_mode = "blocks".into();
        req.transfer_amount = 100;
        let params = client_params_for(&req);
        assert_eq!(params.blocks_to_send, Some(100));
        assert_eq!(params.bytes_to_send, None);
        // 默认按时长：两者皆不设置
        let params = client_params_for(&request("tcp-single", "client", "tcp"));
        assert_eq!(params.bytes_to_send, None);
        assert_eq!(params.blocks_to_send, None);
    }

    #[test]
    fn transfer_mode_validation_rules() {
        let mut req = request("tcp-single", "client", "tcp");
        // 按量模式数量必须大于 0
        req.transfer_mode = "bytes".into();
        req.transfer_amount = 0;
        assert!(validate(&req).is_err());
        req.transfer_amount = 1;
        assert!(validate(&req).is_ok());
        // 非法结束条件直接拒绝
        req.transfer_mode = "packets".into();
        assert!(validate(&req).is_err());
        // 按量模式忽略时长校验：duration 为 0 也应通过（time 模式则拒绝）
        req.transfer_mode = "time".into();
        req.duration = 0;
        assert!(validate(&req).is_err());
        req.transfer_mode = "bytes".into();
        req.transfer_amount = 1;
        assert!(validate(&req).is_ok());
        req.duration = 10;
        // 预热与按量互斥
        req.omit_secs = 1;
        assert!(validate(&req).is_err());
        req.transfer_mode = "time".into();
        assert!(validate(&req).is_ok());
        req.omit_secs = 0;
    }

    #[test]
    fn dscp_range_validated() {
        let mut req = request("tcp-single", "client", "tcp");
        req.dscp = 63;
        assert!(validate(&req).is_ok());
        req.dscp = 64;
        assert!(validate(&req).is_err());
    }

    #[test]
    fn congestion_and_dont_fragment_map_through_params() {
        let mut req = request("udp-bandwidth", "client", "udp");
        req.congestion_algo = "  bbr  ".into();
        req.udp_dont_fragment = true;
        let params = client_params_for(&req);
        // 算法去掉首尾空白；空字符串 = 不设置
        assert_eq!(params.congestion_algo.as_deref(), Some("bbr"));
        assert!(params.udp_dont_fragment);
        // 默认：算法不设置、DF 关闭
        let params = client_params_for(&request("udp-bandwidth", "client", "udp"));
        assert_eq!(params.congestion_algo, None);
        assert!(!params.udp_dont_fragment);
    }

    #[test]
    fn mptcp_maps_through_params() {
        let mut req = request("tcp-single", "client", "tcp");
        req.mptcp = true;
        assert!(client_params_for(&req).mptcp);
        // 默认关闭
        assert!(!client_params_for(&request("tcp-single", "client", "tcp")).mptcp);
    }

    #[test]
    fn byte_items_force_transfer_mode() {
        // tcp-bytes：即使全局为按时长，也强制 bytes（数量取全局 transfer_amount）
        let mut req = request("tcp-bytes", "client", "tcp");
        req.transfer_mode = "time".into();
        req.transfer_amount = 5;
        let params = client_params_for(&req);
        assert_eq!(params.bytes_to_send, Some(5_000_000));
        assert_eq!(params.blocks_to_send, None);
        // udp-bytes 同款；tcp-blocks 强制 blocks
        let params = client_params_for(&request("udp-bytes", "client", "udp"));
        assert_eq!(params.bytes_to_send, Some(0)); // amount 为 0 时仍强制 bytes（数量校验在 validate）
        let params = client_params_for(&request("tcp-blocks", "client", "tcp"));
        assert_eq!(params.blocks_to_send, Some(0));
        // 普通项不受影响
        let params = client_params_for(&request("tcp-single", "client", "tcp"));
        assert_eq!(params.bytes_to_send, None);
    }

    #[test]
    fn feature_items_force_flags() {
        // tcp-mptcp / udp-df：无论全局开关，测试项强制对应标志
        let req = request("tcp-mptcp", "client", "tcp");
        assert!(client_params_for(&req).mptcp);
        let req = request("udp-df", "client", "udp");
        assert!(client_params_for(&req).udp_dont_fragment);
        // 普通项跟随全局开关
        let mut req = request("tcp-single", "client", "tcp");
        req.mptcp = false;
        assert!(!client_params_for(&req).mptcp);
    }

    #[test]
    fn byte_items_require_amount_in_validate() {
        // 按量测试项选中但全局按量数量为 0：必须拒绝（即使全局 mode 为 time）
        let mut req = request("tcp-bytes", "client", "tcp");
        req.transfer_mode = "time".into();
        req.transfer_amount = 0;
        assert!(validate(&req).is_err());
        req.transfer_amount = 1;
        assert!(validate(&req).is_ok());
        // 按量项与预热互斥
        req.omit_secs = 1;
        assert!(validate(&req).is_err());
        // 普通项 + 全局按量模式下：按量项校验仍生效
        let mut mixed = request("tcp-single", "client", "tcp");
        mixed.transfer_amount = 2;
        assert!(validate(&mixed).is_ok());
    }
}
