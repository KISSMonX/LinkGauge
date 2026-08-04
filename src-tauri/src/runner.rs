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
        Arc, Mutex,
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
    if locale == "en" { en } else { zh }
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
    let spawned_id = session_id.clone();
    if request.task_id == "ping" {
        let cancelled = Arc::new(AtomicBool::new(false));
        state
            .sessions
            .lock()
            .await
            .insert(session_id.clone(), SessionSignal::Ping(cancelled.clone()));
        tauri::async_runtime::spawn(async move {
            run_ping(app, spawned_id.clone(), request, cancelled, pids).await;
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
            run_engine_server(app, spawned_id.clone(), request, rx).await;
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
            run_engine_client(app, spawned_id.clone(), request, rx).await;
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
    // 服务端模式不需要持续时间（duration 恒为 0），只校验输出周期
    if request.mode != "server" && request.duration == 0 {
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
    /// 带宽限制（bps），None 表示不限制
    bandwidth_bps: Option<u64>,
    interval: f64,
    bind_address: Option<String>,
}

fn client_params_for(request: &TestRequest) -> ClientParams {
    // 协议由任务类型推断：udp-* 为 UDP，其余（含 stress）为 TCP，
    // 使 TCP/UDP 测试项可以在同一队列中混合执行
    let protocol = if request.task_id.starts_with("udp-") {
        TransportProtocol::Udp
    } else {
        TransportProtocol::Tcp
    };
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
        bandwidth_bps: (request.bandwidth > 0).then_some(request.bandwidth * 1_000_000),
        interval: request.interval as f64,
        bind_address: (!request.local_ip.trim().is_empty()).then(|| request.local_ip.clone()),
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
    let header = tr_format!(
        &request.locale,
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
            tr(&request.locale, "不限制", "unlimited").into()
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
            &request.locale,
            "执行：riperf3 -c {}（内嵌引擎）",
            "Running: riperf3 -c {} (embedded engine)",
            request.server_ip
        ),
    );

    let params = client_params_for(&request);
    // 实时指标回调：每输出周期触发一次，发出 metric 事件并写入日志
    let hook_app = app.clone();
    let hook_session = session_id.clone();
    let hook_task = request.task_id.clone();
    let hook_log = log.clone();
    let hook_locale = request.locale.clone();
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
            &format!("[INFO] {}", format_interval_line(&hook_locale, second, sum)),
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
            let session_id = session_id.clone();
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
    if let Some(bps) = params.bandwidth_bps {
        builder = builder.bandwidth(bps);
    }
    if let Some(addr) = &params.bind_address {
        builder = builder.bind_address(addr);
    }
    let client = match builder.build() {
        Ok(client) => client,
        Err(error) => {
            fail_engine(
                &app,
                &session_id,
                &request.task_id,
                &log,
                &request.locale,
                &tr_format!(
                    &request.locale,
                    "测试配置无效：{}",
                    "Invalid test configuration: {}",
                    error
                ),
            )
            .await;
            return;
        }
    };

    let outcome = client.run().await;
    finish_engine(&app, &session_id, &request.task_id, &log, &request, outcome).await;
}

// ---------------------------------------------------------------------------
// riperf3 服务端任务：持续监听直到手动停止
// ---------------------------------------------------------------------------

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
    append_log(
        &log,
        &tr_format!(
            &request.locale,
            "引擎: riperf3 {}（纯 Rust 内置，无需安装 iperf3）\n参数: 绑定 {}，监听端口 {}，持续服务\n\n",
            "Engine: riperf3 {} (pure Rust, built-in, no iperf3 needed)\nParams: bind {}, listen port {}, serving continuously\n\n",
            riperf3::VERSION,
            bind_display,
            request.port
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
    server_builder = server_builder.on_interval({
        let serving = serving.clone();
        let latest = latest.clone();
        move |interval: &riperf3::json_report::Interval| {
            serving.store(true, Ordering::Relaxed);
            if interval.sum.omitted {
                return;
            }
            let sum = &interval.sum;
            *latest.lock().unwrap() = Some(ServerInterval {
                bandwidth_mbps: sum.bits_per_second / 1_000_000.0,
                transfer_mb: sum.bytes as f64 / 1_000_000.0,
                jitter_ms: sum.jitter_ms.unwrap_or(0.0),
                loss_percent: sum.lost_percent.unwrap_or(0.0),
                retransmits: sum.retransmits.unwrap_or(0).max(0) as u64,
                ..Default::default()
            });
        }
    });
    // 客户端一建立控制连接就实时广播对端地址（服务端概览「对端=客户端」无需等待测试结束）
    server_builder = server_builder.on_connect({
        let app = app.clone();
        let session_id = session_id.clone();
        let task_id = request.task_id.clone();
        let log = log.clone();
        let locale = request.locale.clone();
        move |addr| {
            let host = addr.ip().to_canonical().to_string();
            let port = addr.port();
            let connected = tr_format!(
                &locale,
                "客户端 {}:{} 已连接",
                "Client {}:{} connected",
                host,
                port
            );
            append_log(&log, &format!("[INFO] {connected}"));
            emit_log(&app, &session_id, &task_id, "INFO", connected);
            let payload = format!(r#"{{"serving":true,"peerIp":"{}","peerPort":{}}}"#, host, port);
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
                &request.locale,
                &tr_format!(
                    &request.locale,
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
                &request.locale,
                &tr_format!(
                    &request.locale,
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
                &request.locale,
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
            &request.locale,
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
        let locale = request.locale.clone();
        tauri::async_runtime::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(interval_secs));
            ticker.tick().await; // 跳过立即触发的第一次
            let started = std::time::Instant::now();
            loop {
                ticker.tick().await;
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
                        s.bandwidth_mbps, s.transfer_mb, s.jitter_ms, s.loss_percent, s.retransmits
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
                *latest.lock().unwrap() = match &peer {
                    Some((ip, p)) => Some(ServerInterval {
                        peer_ip: ip.clone(),
                        peer_port: *p,
                        ..Default::default()
                    }),
                    None => None,
                };
                if outcome.termination == Termination::Interrupted {
                    heartbeat.abort();
                    append_log(
                        &log,
                        &format!(
                            "\n{}",
                            tr(&request.locale, "测试结果: 服务端已停止（手动停止）", "Result: server stopped (manual stop)")
                        ),
                    );
                    emit_log(
                        &app,
                        &session_id,
                        &request.task_id,
                        "INFO",
                        tr(&request.locale, "服务端已停止（手动停止）", "Server stopped (manual stop)").into(),
                    );
                    finish_ok(&app, &session_id, &request.task_id, &log, "stopped").await;
                    return;
                }
                // 单次测试结束：写入汇总并继续监听
                let peer_text = peer
                    .as_ref()
                    .map(|(ip, p)| format!("{ip}:{p}"))
                    .unwrap_or_else(|| tr(&request.locale, "未知地址", "unknown address").to_string());
                append_log(
                    &log,
                    &format!(
                        "\n[INFO] {}",
                        tr_format!(
                            &request.locale,
                            "客户端 {} 完成测试，汇总如下：",
                            "Client {} finished a test, summary:",
                            peer_text
                        )
                    ),
                );
                append_engine_summary(&log, &outcome.report, &request.locale);
                emit_log(
                    &app,
                    &session_id,
                    &request.task_id,
                    "INFO",
                    tr_format!(
                        &request.locale,
                        "一次测试完成（客户端 {}），继续监听…",
                        "Test finished (client {}), still listening…",
                        peer_text
                    ),
                );
            }
            Err(error) => {
                serving.store(false, Ordering::Relaxed);
                *latest.lock().unwrap() = None;
                // 空闲时收到停止信号返回 Aborted，正常退出
                if matches!(error, riperf3::RiperfError::Aborted(_)) {
                    heartbeat.abort();
                    append_log(
                        &log,
                        &format!(
                            "\n{}",
                            tr(&request.locale, "测试结果: 服务端已停止（手动停止）", "Result: server stopped (manual stop)")
                        ),
                    );
                    emit_log(
                        &app,
                        &session_id,
                        &request.task_id,
                        "INFO",
                        tr(&request.locale, "服务端已停止（手动停止）", "Server stopped (manual stop)").into(),
                    );
                    finish_ok(&app, &session_id, &request.task_id, &log, "stopped").await;
                    return;
                }
                let warn = tr_format!(
                    &request.locale,
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
        tr_format!(
            &request.locale,
            "执行：ping {}",
            "Running: ping {}",
            args.join(" ")
        ),
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
                &request.locale,
                "启动 ping 失败：{}",
                "Failed to start ping: {}",
                error
            );
            append_log(&log, &format!("[ERROR] {message}"));
            fail_engine(&app, &session_id, &request.task_id, &log, &request.locale, &message).await;
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
            tr(&request.locale, "测试结果", "Result"),
            if final_success {
                tr(&request.locale, "完成", "completed")
            } else {
                tr(&request.locale, "未完成", "incomplete")
            }
        ),
    );
    if final_stopped {
        finish_ok(&app, &session_id, &request.task_id, &log, "stopped").await;
    } else if final_success {
        finish_ok(&app, &session_id, &request.task_id, &log, "success").await;
    } else {
        let message = tr(&request.locale, "ping 测试进程异常退出", "Ping process exited unexpectedly").to_string();
        fail_engine(&app, &session_id, &request.task_id, &log, &request.locale, &message).await;
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
            tr_format!(&request.locale, "无法创建日志目录：{}", "Cannot create log directory: {}", error),
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
                tr_format!(&request.locale, "无法创建日志文件：{}", "Cannot create log file: {}", error),
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
    outcome: Result<riperf3::RunOutcome, riperf3::RiperfError>,
) {
    match outcome {
        Err(error) => {
            let message = tr_format!(
                &request.locale,
                "测试失败：{}",
                "Test failed: {}",
                error
            );
            fail_engine(app, session_id, task_id, log, &request.locale, &message).await;
        }
        Ok(outcome) => {
            let report = &outcome.report;
            append_log(
                log,
                &format!(
                    "\n[INFO] {}",
                    tr(&request.locale, "测试结束，最终汇总：", "Test finished, final summary:")
                ),
            );
            append_engine_summary(log, report, &request.locale);
            emit_final_metric(app, session_id, task_id, report, request.duration as i64);
            match outcome.termination {
                Termination::Completed => {
                    append_log(
                        log,
                        &format!(
                            "\n{}",
                            tr(&request.locale, "测试结果: 完成", "Result: completed")
                        ),
                    );
                    finish_ok(app, session_id, task_id, log, "success").await;
                }
                Termination::Interrupted => {
                    append_log(
                        log,
                        &format!(
                            "\n{}",
                            tr(&request.locale, "测试结果: 手动停止", "Result: manual stop")
                        ),
                    );
                    finish_ok(app, session_id, task_id, log, "stopped").await;
                }
                Termination::ServerTerminated => {
                    let message = tr(&request.locale, "服务端主动终止了测试", "The server terminated the test").to_string();
                    fail_engine(app, session_id, task_id, log, &request.locale, &message).await;
                }
                Termination::ServerError(msg) => {
                    let message = tr_format!(
                        &request.locale,
                        "服务端返回错误：{}",
                        "Server returned an error: {}",
                        msg
                    );
                    fail_engine(app, session_id, task_id, log, &request.locale, &message).await;
                }
                other => {
                    let message = tr_format!(
                        &request.locale,
                        "测试异常结束：{:?}",
                        "Test ended abnormally: {:?}",
                        other
                    );
                    fail_engine(app, session_id, task_id, log, &request.locale, &message).await;
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
            line.push_str(&tr_format!(locale, ", 重传 {}", ", retransmits {}", retransmits));
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
fn format_interval_line(locale: &str, second: i64, sum: &riperf3::json_report::IntervalSum) -> String {
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
        line.push_str(&tr_format!(locale, ", 重传 {}", ", retransmits {}", retransmits));
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
    use super::client_params_for;
    use crate::models::TestRequest;
    use riperf3::TransportProtocol;

    fn request(task_id: &str, mode: &str, protocol: &str) -> TestRequest {
        TestRequest {
            task_id: task_id.into(),
            mode: mode.into(),
            protocol: protocol.into(),
            server_ip: "192.168.1.100".into(),
            local_ip: "192.168.1.50".into(),
            port: 5201,
            duration: 10,
            parallel: 4,
            bandwidth: 0,
            packet_length: 1024,
            udp_packet_length: 8192,
            interval: 1,
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
            None
        );
    }

    #[test]
    fn bandwidth_mbps_to_bps() {
        let mut req = request("tcp-single", "client", "tcp");
        req.bandwidth = 100;
        assert_eq!(client_params_for(&req).bandwidth_bps, Some(100_000_000));
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
}
