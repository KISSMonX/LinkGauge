//! riperf3 服务端任务：持续监听直到手动停止。
//!
//! 从 runner.rs 拆分，使服务端逻辑可独立阅读。

use crate::models::TestRequest;
use crate::runner::{
    append_engine_summary, append_log, current_locale, emit_log, fail_engine, finish_ok,
    task_label, tr, SessionLog,
};
use riperf3::{RiperfError, RunOutcome, ServerBuilder, Termination};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex, RwLock,
};
use tauri::{AppHandle, Emitter};
use tokio::sync::watch;
use tokio::time::Duration;

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
/// 同时记录最近一次连接的客户端地址（服务端概览"对端=客户端"的数据源）
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

pub(crate) async fn run_engine_server<R: tauri::Runtime>(
    app: AppHandle<R>,
    session_id: String,
    request: TestRequest,
    rx: watch::Receiver<Option<String>>,
    locale_handle: Arc<RwLock<String>>,
) {
    let task_name = task_label(&request.locale, &request.task_id);
    let local_ip = if request.local_ip.trim().is_empty() {
        local_ip_address::local_ip()
            .map(|ip| ip.to_string())
            .unwrap_or_else(|_| "127.0.0.1".into())
    } else {
        request.local_ip.clone()
    };
    let Some(log) =
        crate::runner::setup_log(&app, &session_id, &request, &local_ip, task_name).await
    else {
        return;
    };
    let bind_display = if request.bind_ip.trim().is_empty() {
        "0.0.0.0（所有网卡）".to_string()
    } else {
        request.bind_ip.clone()
    };
    // 界面语言运行时可变：头部日志用启动时语言，循环内每次迭代实时读取
    let locale = current_locale(&locale_handle);
    // 认证启用时在头部日志标注：服务端持有私钥与用户文件，客户端需按"客户端 → 认证"配置
    let auth_display = if request.server_auth_enabled {
        crate::tr_format!(
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
        limits.push(crate::tr_format!(
            &locale,
            "空闲超时 {}s",
            "idle timeout {}s",
            request.server_idle_timeout
        ));
    }
    if request.server_max_duration > 0 {
        limits.push(crate::tr_format!(
            &locale,
            "单测上限 {}s",
            "max duration {}s",
            request.server_max_duration
        ));
    }
    if request.server_bitrate_limit_mbps > 0 {
        limits.push(crate::tr_format!(
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
        &crate::tr_format!(
            locale,
            "引擎: riperf3 {}（纯 Rust 内置，无需安装 iperf3）\n参数: 绑定 {}，监听端口 {}，持续服务{}{}\n",
            "Engine: riperf3 {} (pure Rust, built-in, no iperf3 needed)\nParams: bind {}, listen port {}, serving continuously{}{}\n",
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
    // 客户端表现为"访问被拒"，服务端日志记录具体原因
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
    // 监听循环据此按"空闲超时停止"退出而非继续监听
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
    // 服务端统计采样间隔（-i，本地补丁）：与界面"日志输出间隔"一致。
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
            let mut guard = latest.lock().unwrap_or_else(|e| e.into_inner());
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
    // 客户端一建立控制连接就实时广播对端地址（服务端概览"对端=客户端"无需等待测试结束）
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
            let mut snapshot_guard = latest.lock().unwrap_or_else(|e| e.into_inner());
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
            let connected = crate::tr_format!(
                &locale,
                "客户端 {}:{} 已连接",
                "Client {}:{} connected",
                host,
                port
            );
            append_log(&log, &format!("[INFO] {connected}"));
            emit_log(&app, &session_id, &task_id, "INFO", connected);
            let payload = serde_json::json!({
                "serving": true,
                "peerIp": host,
                "peerPort": port
            })
            .to_string();
            let _ = app.emit(
                "test-event",
                crate::models::TestEvent {
                    session_id: session_id.clone(),
                    task_id: task_id.clone(),
                    event_type: "status".into(),
                    status: None,
                    level: None,
                    message: Some(payload),
                    metric: None,
                    log_path: None,
                    fatal: None,
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
                &crate::tr_format!(
                    &locale,
                    "服务端配置无效：{}",
                    "Invalid server configuration: {}",
                    error
                ),
                false,
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
                &crate::tr_format!(
                    &locale,
                    "无法监听端口 {}：{}",
                    "Cannot listen on port {}: {}",
                    request.port,
                    error
                ),
                false,
            )
            .await;
            return;
        }
    };
    append_log(
        &log,
        &format!(
            "[INFO] {}",
            crate::tr_format!(
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
        crate::tr_format!(
            locale,
            "服务端已就绪，监听 {}:{}",
            "Server ready, listening on {}:{}",
            bind_display,
            request.port
        ),
    );

    // 周期状态输出：按"日志输出间隔"每隔 N 秒写一次运行日志与统计信息（并发于监听循环）
    let interval_secs = request.interval.max(1);
    let completed = Arc::new(AtomicU64::new(0));
    let bind_short = if request.bind_ip.trim().is_empty() {
        "0.0.0.0".to_string()
    } else {
        request.bind_ip.clone()
    };
    let port = request.port;
    let heartbeat = tauri::async_runtime::spawn(server_heartbeat(
        log.clone(),
        app.clone(),
        session_id.clone(),
        request.task_id.clone(),
        completed.clone(),
        serving.clone(),
        latest.clone(),
        locale_handle.clone(),
        interval_secs,
        bind_short,
        port,
    ));

    loop {
        // 每次迭代实时读取界面语言（切换语言后服务端日志立即跟随）
        let locale = current_locale(&locale_handle);
        match bound.run_once().await {
            Ok(outcome) => {
                if !handle_server_connection(
                    &app,
                    &session_id,
                    &request.task_id,
                    &log,
                    &locale,
                    outcome,
                    &serving,
                    &completed,
                    &latest,
                    &heartbeat,
                )
                .await
                {
                    return;
                }
            }
            Err(error) => {
                if !handle_server_listen_error(
                    &app,
                    &session_id,
                    &request.task_id,
                    &log,
                    &locale,
                    error,
                    &serving,
                    &latest,
                    &heartbeat,
                )
                .await
                {
                    return;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 辅助函数：从 run_engine_server 拆分，降低函数圈复杂度
// ---------------------------------------------------------------------------

/// 周期性心跳：按 interval_secs 间隔输出服务端运行状态日志与统计事件。
async fn server_heartbeat<R: tauri::Runtime>(
    log: SessionLog,
    app: AppHandle<R>,
    session_id: String,
    task_id: String,
    completed: Arc<AtomicU64>,
    serving: Arc<AtomicBool>,
    latest: Arc<Mutex<Option<ServerInterval>>>,
    locale_handle: Arc<RwLock<String>>,
    interval_secs: u64,
    bind_short: String,
    port: u16,
) {
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
        let snapshot = latest.lock().unwrap_or_else(|e| e.into_inner()).clone();
        // 文本日志行（写文件 + 广播）
        let status_text = if is_serving {
            tr(&locale, "测试进行中", "test in progress")
        } else {
            tr(&locale, "空闲", "idle")
        };
        let message = crate::tr_format!(
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
        // 结构化统计事件：供服务端窗口的概览与实时曲线使用
        let mut payload = serde_json::json!({
            "uptime": uptime,
            "completed": done,
            "serving": is_serving,
        });
        if let Some(s) = &snapshot {
            let obj = payload.as_object_mut().unwrap();
            obj.insert(
                "bandwidthMbps".into(),
                serde_json::json!(safe_f64(s.bandwidth_mbps)),
            );
            obj.insert(
                "transferMb".into(),
                serde_json::json!(safe_f64(s.transfer_mb)),
            );
            obj.insert("jitterMs".into(), serde_json::json!(safe_f64(s.jitter_ms)));
            obj.insert(
                "lossPercent".into(),
                serde_json::json!(safe_f64(s.loss_percent)),
            );
            obj.insert("retransmits".into(), serde_json::json!(s.retransmits));
            if !s.peer_ip.is_empty() {
                obj.insert("peerIp".into(), serde_json::json!(s.peer_ip));
                obj.insert("peerPort".into(), serde_json::json!(s.peer_port));
            }
        }
        let payload = payload.to_string();
        let _ = app.emit(
            "test-event",
            crate::models::TestEvent {
                session_id: session_id.clone(),
                task_id: task_id.clone(),
                event_type: "status".into(),
                status: None,
                level: None,
                message: Some(payload),
                metric: None,
                log_path: None,
                fatal: None,
            },
        );
    }
}

/// 处理一次成功的服务端连接测试结果。返回 `true` 表示继续监听。
async fn handle_server_connection<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: &str,
    task_id: &str,
    log: &SessionLog,
    locale: &str,
    outcome: RunOutcome,
    serving: &AtomicBool,
    completed: &AtomicU64,
    latest: &Mutex<Option<ServerInterval>>,
    heartbeat: &tauri::async_runtime::JoinHandle<()>,
) -> bool {
    serving.store(false, Ordering::Relaxed);
    completed.fetch_add(1, Ordering::Relaxed);
    // 记录本次连接的客户端地址（服务端概览"对端=客户端"的数据源）
    let peer = outcome
        .report
        .start
        .connected
        .first()
        .map(|c| (c.remote_host.clone(), c.remote_port));
    // 测试结束（客户端已断开）：清空对端地址与统计，心跳事件不再携带
    *latest.lock().unwrap_or_else(|e| e.into_inner()) = None;
    if outcome.termination == Termination::Interrupted {
        heartbeat.abort();
        append_log(
            log,
            tr(
                locale,
                "测试结果: 服务端已停止（手动停止）",
                "Result: server stopped (manual stop)",
            ),
        );
        emit_log(
            app,
            session_id,
            task_id,
            "INFO",
            tr(
                locale,
                "服务端已停止（手动停止）",
                "Server stopped (manual stop)",
            )
            .into(),
        );
        finish_ok(app, session_id, task_id, log, "stopped").await;
        return false;
    }
    // 单次测试结束：写入汇总并继续监听
    let peer_text = peer
        .as_ref()
        .map(|(ip, p)| format!("{ip}:{p}"))
        .unwrap_or_else(|| tr(locale, "未知地址", "unknown address").to_string());
    append_log(
        log,
        &format!(
            "[INFO] {}",
            crate::tr_format!(
                locale,
                "客户端 {} 完成测试，汇总如下：",
                "Client {} finished a test, summary:",
                peer_text
            )
        ),
    );
    append_engine_summary(log, &outcome.report, locale);
    emit_log(
        app,
        session_id,
        task_id,
        "INFO",
        crate::tr_format!(
            locale,
            "一次测试完成（客户端 {}），继续监听…",
            "Test finished (client {}), still listening…",
            peer_text
        ),
    );
    true
}

/// 处理服务端监听循环中的错误。返回 `true` 表示继续监听。
async fn handle_server_listen_error<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: &str,
    task_id: &str,
    log: &SessionLog,
    locale: &str,
    error: RiperfError,
    serving: &AtomicBool,
    latest: &Mutex<Option<ServerInterval>>,
    heartbeat: &tauri::async_runtime::JoinHandle<()>,
) -> bool {
    serving.store(false, Ordering::Relaxed);
    *latest.lock().unwrap_or_else(|e| e.into_inner()) = None;
    // 空闲时收到停止信号返回 Aborted，正常退出；idle_timeout 到期
    // 同样以 Aborted("idle timeout") 返回（one_off 下退出而非重启），
    // 按原因区分日志文案
    if let RiperfError::Aborted(msg) = &error {
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
        append_log(log, tr(locale, result_zh, result_en));
        emit_log(
            app,
            session_id,
            task_id,
            "INFO",
            tr(locale, reason_zh, reason_en).into(),
        );
        finish_ok(app, session_id, task_id, log, "stopped").await;
        return false;
    }
    let warn = crate::tr_format!(
        locale,
        "一次连接处理失败：{}，继续监听",
        "Failed to handle a connection: {}, still listening",
        error
    );
    append_log(log, &format!("[WARN] {warn}"));
    emit_log(app, session_id, task_id, "WARN", warn);
    true
}
