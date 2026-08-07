//! ping 任务（保留系统进程调用，riperf3 不支持 ICMP）。
//!
//! 从 runner.rs 拆分，使 ping 逻辑可独立阅读与测试。

use crate::client::{client_task_timeout, ClientTaskResult};
use crate::models::{MetricPoint, TestRequest};
use crate::runner::{
    append_log, current_locale, emit_log, emit_metric, fail_engine, finish_ok, task_label, tr,
};
use regex::Regex;
use std::collections::HashSet;
use std::process::Stdio;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, RwLock,
};
use tauri::AppHandle;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::{mpsc, Mutex as AsyncMutex},
    time::{sleep, Duration},
};

// ---------------------------------------------------------------------------
// ping 任务（保留系统进程调用，riperf3 不支持 ICMP）
// ---------------------------------------------------------------------------

/// 已启动的 ping 子进程及其 I/O 通道。
struct PingChild {
    child: tokio::process::Child,
    rx: tokio::sync::mpsc::Receiver<(String, bool)>,
    pid: Option<u32>,
}

/// 启动 ping 子进程，挂接 stdout / stderr 读取器。
fn spawn_ping(args: &[String]) -> Result<PingChild, String> {
    let mut command = Command::new("ping");
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut child = command.spawn().map_err(|error| format!("{error}"))?;
    let pid = child.id();
    let (tx, rx) = mpsc::channel::<(String, bool)>(128);
    if let Some(stdout) = child.stdout.take() {
        let tx = tx.clone();
        tokio::spawn(read_lines(stdout, tx, false));
    }
    if let Some(stderr) = child.stderr.take() {
        let tx = tx.clone();
        tokio::spawn(read_lines(stderr, tx, true));
    }
    drop(tx);
    Ok(PingChild { child, rx, pid })
}

/// 处理 ping 退出结果：清理 PID、记录结果日志、发送结束事件。
async fn finalize_ping<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: &str,
    task_id: &str,
    log: &crate::runner::SessionLog,
    locale: &str,
    final_success: bool,
    final_stopped: bool,
    final_timed_out: bool,
) -> ClientTaskResult {
    if final_timed_out {
        let message = tr(
            locale,
            "Ping 超过后端硬时限，已终止并继续下一项",
            "Ping exceeded the backend hard timeout and was stopped; continuing with the next item",
        );
        fail_engine(app, session_id, task_id, log, locale, message, false).await;
        ClientTaskResult::Failed
    } else if final_stopped {
        finish_ok(app, session_id, task_id, log, "stopped").await;
        ClientTaskResult::Stopped
    } else if final_success {
        finish_ok(app, session_id, task_id, log, "success").await;
        ClientTaskResult::Success
    } else {
        let message = tr(
            locale,
            "ping 测试进程异常退出",
            "Ping process exited unexpectedly",
        )
        .to_string();
        fail_engine(app, session_id, task_id, log, locale, &message, false).await;
        ClientTaskResult::Failed
    }
}

pub(crate) async fn run_ping<R: tauri::Runtime>(
    app: AppHandle<R>,
    session_id: String,
    request: TestRequest,
    cancelled: Arc<AtomicBool>,
    child_pids: Arc<AsyncMutex<HashSet<u32>>>,
    locale_handle: Arc<RwLock<String>>,
) -> ClientTaskResult {
    let task_name = task_label(&request.locale, &request.task_id);
    // 界面语言运行时可变：ping 日志实时读取当前语言
    let locale = current_locale(&locale_handle);
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
    let count_flag = if cfg!(windows) { "-n" } else { "-c" };
    let args = vec![count_flag.into(), "4".into(), request.server_ip.clone()];
    let exec_line = crate::tr_format!(locale, "执行：ping {}", "Running: ping {}", args.join(" "));
    append_log(&log, &format!("[INFO] {exec_line}"));
    emit_log(&app, &session_id, &request.task_id, "INFO", exec_line);

    let PingChild {
        mut child,
        mut rx,
        pid,
    } = match spawn_ping(&args) {
        Ok(pc) => pc,
        Err(error) => {
            let message = crate::tr_format!(
                locale,
                "启动 ping 失败：{}",
                "Failed to start ping: {}",
                error
            );
            append_log(&log, &format!("[ERROR] {message}"));
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
    if let Some(pid) = pid {
        child_pids.lock().await.insert(pid);
    }
    let mut ping_sample = 0;
    let mut final_success = false;
    let mut final_stopped = false;
    let mut final_timed_out = false;
    let hard_timeout = sleep(client_task_timeout(&request));
    tokio::pin!(hard_timeout);
    loop {
        tokio::select! {
            status = child.wait() => {
                final_success = status.map(|value| value.success()).unwrap_or(false);
                break;
            }
            line = rx.recv() => {
                if let Some((line, is_error)) = line {
                    let level = if is_error { "WARN" } else { "INFO" };
                    let output_line = format!("[{level}] {line}");
                    append_log(&log, &output_line);
                    emit_log(&app, &session_id, &request.task_id, level, line.clone());
                    if let Some(metric) = parse_ping_metric(&line, ping_sample + 1) {
                        ping_sample += 1;
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
            _ = &mut hard_timeout => {
                final_timed_out = true;
                let _ = child.kill().await;
                let _ = child.wait().await;
                break;
            }
        }
    }
    if let Some(pid) = pid {
        child_pids.lock().await.remove(&pid);
    }
    append_log(
        &log,
        &format!(
            "{}: {}",
            tr(&locale, "测试结果", "Result"),
            if final_success {
                tr(&locale, "完成", "completed")
            } else {
                tr(&locale, "未完成", "incomplete")
            }
        ),
    );
    finalize_ping(
        &app,
        &session_id,
        &request.task_id,
        &log,
        &locale,
        final_success,
        final_stopped,
        final_timed_out,
    )
    .await
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

/// 每次调用不再重新编译正则：ping 输出解析在热点路径上（ping 的 stdout 每行都调），
/// 用 LazyLock 确保只编译一次。
static PING_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r"(?i)(?:time|时间)[=<＝]\s*(\d+(?:\.\d+)?)\s*ms")
        .expect("ping time regex is statically valid")
});

pub(crate) fn parse_ping_metric(line: &str, second: i64) -> Option<MetricPoint> {
    PING_RE.captures(line).map(|c| MetricPoint {
        second,
        jitter_ms: c[1].parse().unwrap_or(0.0),
        ..Default::default()
    })
}
