//! riperf3 客户端任务：参数映射、引擎构造与执行。
//!
//! 从 runner.rs 拆分，使客户端逻辑可独立阅读与测试。

use crate::models::{MetricPoint, TestEvent, TestRequest};
use crate::runner::{
    append_log, current_locale, emit_log, emit_metric, fail_engine, finish_engine, task_label, tr,
    SessionLog,
};
use riperf3::{ClientBuilder, TransportProtocol};
use std::{
    sync::{Arc, RwLock},
};
use tauri::{AppHandle, Emitter};
use tokio::sync::watch;
use tokio::time::Duration;

// ---------------------------------------------------------------------------
// riperf3 客户端任务
// ---------------------------------------------------------------------------

/// 客户端参数映射（纯函数，便于单测）——与旧 CLI 参数一一对应
#[derive(Debug, PartialEq)]
pub(crate) struct ClientParams {
    pub(crate) protocol: TransportProtocol,
    pub(crate) duration: u32,
    pub(crate) num_streams: u32,
    pub(crate) blksize: Option<usize>,
    pub(crate) reverse: bool,
    pub(crate) bidir: bool,
    /// 带宽限制（bps），0 = 不限制。必须无条件下发给引擎，不能「为 0 就不设置」：
    /// riperf3 未调用 bandwidth() 时 UDP 会套用 iperf3 的 UDP_RATE 默认值
    /// （1 Mibit/s，见 vendor/riperf3 的 utils.rs），导致界面选「不限制」的 UDP
    /// 测试实际被限速到约 1 Mbps；显式传 0 才是引擎语义下的不限制。
    pub(crate) bandwidth_bps: u64,
    pub(crate) interval: f64,
    pub(crate) bind_address: Option<String>,
    /// 预热秒数（0 = 不预热）：on_interval 回调已跳过 omitted 区间，仅影响统计口径
    pub(crate) omit_secs: u32,
    /// 套接字缓冲区（KB，0 = 自动）
    pub(crate) window_kb: u32,
    /// 数据流源端口（0 = 自动；第 i 条流绑定 cport+i，对应 iperf3 --cport）
    pub(crate) cport: u16,
    /// IP 协议族（0 = 自动，4 / 6 = 强制）
    pub(crate) ip_version: u8,
    /// 测试结束后拉取服务端视角的输出（--get-server-output）
    pub(crate) get_server_output: bool,
    /// 按量测试：bytes（-n）/ blocks（-k）优先于时长，二者互斥（引擎语义）
    pub(crate) bytes_to_send: Option<u64>,
    pub(crate) blocks_to_send: Option<u64>,
    /// DSCP 值（0 = 不设置，1-63 映射到 TOS 高 6 位）
    pub(crate) dscp: u32,
    /// TCP 拥塞控制算法（None = 默认；仅 Linux/FreeBSD 生效）
    pub(crate) congestion_algo: Option<String>,
    /// UDP 禁止分片标志（仅 IPv4）
    pub(crate) udp_dont_fragment: bool,
    /// MPTCP 多路径（需两端内核支持）
    pub(crate) mptcp: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClientTaskResult {
    Success,
    Failed,
    Fatal,
    Stopped,
}

/// 每项任务的后端硬时限。前端定时器可能随 WebView 被节流或销毁，因此这里只用
/// Tokio 时钟裁决；按量任务根据限速估算传输时间，其余为配置时长加 30 秒余量。
pub(crate) fn client_task_timeout(request: &TestRequest) -> Duration {
    let seconds = match effective_transfer_mode(request) {
        mode @ ("bytes" | "blocks") if request.bandwidth > 0 => {
            let bytes = if mode == "bytes" {
                u128::from(request.transfer_amount).saturating_mul(1_000_000)
            } else {
                u128::from(request.transfer_amount).saturating_mul(u128::from(
                    if request.packet_length == 0 {
                        131_072
                    } else {
                        request.packet_length
                    },
                ))
            };
            let bits_per_second = u128::from(request.bandwidth).saturating_mul(1_000_000);
            let estimated = bytes
                .saturating_mul(8)
                .saturating_add(bits_per_second.saturating_sub(1))
                / bits_per_second.max(1);
            u64::try_from(estimated)
                .unwrap_or(u64::MAX)
                .saturating_add(15)
                .max(30)
        }
        "bytes" | "blocks" => 300,
        _ if request.task_id == "ping" => 30,
        _ => request.duration.saturating_add(30).max(30),
    };
    Duration::from_secs(seconds)
}

/// 按量模式推导：按量测试项（tcp-bytes / udp-bytes / tcp-blocks）强制
/// bytes / blocks，其余取全局 transfer_mode。validate 与 client_params_for
/// 共用同一推导，保证「队列里混排按量项 + 常规项」时各项口径一致。
/// 空串（服务端等不传 transfer_mode 的旧请求，serde 缺省）归一化为 "time"，
/// 非空非法值（如 "packets"）原样返回由调用方拒绝
pub(crate) fn effective_transfer_mode(request: &TestRequest) -> &str {
    match request.task_id.as_str() {
        "tcp-bytes" | "udp-bytes" => "bytes",
        "tcp-blocks" => "blocks",
        _ => {
            let mode = request.transfer_mode.as_str();
            if mode.is_empty() {
                "time"
            } else {
                mode
            }
        }
    }
}

pub(crate) fn client_params_for(request: &TestRequest) -> ClientParams {
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

pub(crate) async fn run_engine_client<R: tauri::Runtime>(
    app: AppHandle<R>,
    session_id: String,
    request: TestRequest,
    rx: watch::Receiver<Option<String>>,
    locale_handle: Arc<RwLock<String>>,
) -> ClientTaskResult {
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
        return ClientTaskResult::Failed;
    };
    // 界面语言运行时可变：日志输出点实时读取，切换语言后立即生效
    let locale = current_locale(&locale_handle);
    let header = crate::tr_format!(
        &locale,
        "引擎: riperf3 {}（纯 Rust 内置，无需安装 iperf3）\n参数: {}, 端口 {}, 时长 {}s, 并发 {}, 带宽 {}, 报文长度 {}, 输出周期 {}s\n",
        "Engine: riperf3 {} (pure Rust, built-in, no iperf3 needed)\nParams: {}, port {}, duration {}s, streams {}, bandwidth {}, packet length {}, interval {}s\n",
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
    // 执行行同时写入运行日志（此前只发事件，运行日志缺执行过程）
    let exec_line = crate::tr_format!(
        &locale,
        "执行：riperf3 -c {}（内嵌引擎）",
        "Running: riperf3 -c {} (embedded engine)",
        request.server_ip
    );
    append_log(&log, &format!("[INFO] {exec_line}"));
    emit_log(&app, &session_id, &request.task_id, "INFO", exec_line);

    let params = client_params_for(&request);
    let builder = engine_client_builder(
        &app,
        &session_id,
        &request,
        &params,
        &log,
        &locale_handle,
        rx,
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
                &crate::tr_format!(
                    &locale,
                    "测试配置无效：{}",
                    "Invalid test configuration: {}",
                    error
                ),
                false,
            )
            .await;
            return ClientTaskResult::Failed;
        }
    };
    let outcome = match tokio::time::timeout(client_task_timeout(&request), client.run()).await {
        Ok(outcome) => outcome,
        Err(_) => {
            let message = crate::tr_format!(
                &locale,
                "任务超过后端硬时限，已终止并继续下一项",
                "The task exceeded the backend hard timeout and was stopped; continuing with the next item"
            );
            fail_engine(
                &app,
                &session_id,
                &request.task_id,
                &log,
                &locale,
                &message,
                false,
            )
            .await;
            return ClientTaskResult::Failed;
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
    .await
}

/// 构造一次 riperf3 客户端 builder。
#[allow(clippy::too_many_arguments)]
pub(crate) fn engine_client_builder<R: tauri::Runtime>(
    app: &AppHandle<R>,
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
        let message =
            crate::runner::format_interval_line(&current_locale(&hook_locale), second, sum);
        let line = format!("[INFO] {message}");
        append_log(&hook_log, &line);
        // 指标不仅驱动图表，也同步到界面日志；长时测试若只显示"开始测试"，
        // 即使引擎正常运行也会被误判为卡死。
        emit_log(&hook_app, &hook_session, &hook_task, "INFO", message);
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
            // 控制连接建立即广播本地端口（客户端"连接状态"展示本次连接的本地端口）
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
                        fatal: None,
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
    // 客户端须用服务端公钥加密"用户名+密码"。未启用时前端已把三项清空，这里自然跳过。
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
