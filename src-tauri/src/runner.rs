use crate::models::{MetricPoint, TestEvent, TestRequest};
use crate::runtime::resolve_iperf_path;
use chrono::Local;
use regex::Regex;
use std::{
    collections::HashMap,
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::{
    fs::{self, OpenOptions},
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::{mpsc, Mutex},
    time::{sleep, Duration},
};
use uuid::Uuid;

#[derive(Default)]
pub struct AppState {
    pub cancellations: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
}

#[tauri::command]
pub async fn start_test(
    app: AppHandle,
    state: State<'_, AppState>,
    request: TestRequest,
) -> Result<String, String> {
    validate(&request)?;
    let session_id = Uuid::new_v4().to_string();
    let cancelled = Arc::new(AtomicBool::new(false));
    state
        .cancellations
        .lock()
        .await
        .insert(session_id.clone(), cancelled.clone());
    let state_map = state.cancellations.clone();
    let spawned_id = session_id.clone();
    tauri::async_runtime::spawn(async move {
        run_process(app, spawned_id.clone(), request, cancelled).await;
        state_map.lock().await.remove(&spawned_id);
    });
    Ok(session_id)
}

#[tauri::command]
pub async fn stop_test(state: State<'_, AppState>, session_id: String) -> Result<(), String> {
    let map = state.cancellations.lock().await;
    let flag = map
        .get(&session_id)
        .ok_or_else(|| "当前测试任务不存在或已经结束".to_string())?;
    flag.store(true, Ordering::SeqCst);
    Ok(())
}

fn validate(request: &TestRequest) -> Result<(), String> {
    if request.duration == 0 || request.interval == 0 {
        return Err("持续时间和输出周期必须大于 0".into());
    }
    if request.server_ip.trim().is_empty() && request.mode != "server" {
        return Err("服务端地址不能为空".into());
    }
    if request.iperf_path.trim().is_empty() {
        return Err("iperf3 可执行文件路径不能为空".into());
    }
    Ok(())
}

async fn run_process(
    app: AppHandle,
    session_id: String,
    request: TestRequest,
    cancelled: Arc<AtomicBool>,
) {
    let task_name = task_label(&request.task_id);
    let stamp = Local::now().format("%Y%m%d%H%M%S").to_string();
    let local_ip = local_ip_address::local_ip()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|_| "127.0.0.1".into());
    let base_name = format!(
        "{}-{}-{}-{}",
        safe_name(&local_ip),
        safe_name(&request.server_ip),
        safe_name(task_name),
        stamp
    );
    let log_dir = app
        .path()
        .app_log_dir()
        .unwrap_or_else(|_| std::env::temp_dir().join("iperf3-gui"))
        .join("tests");
    if let Err(error) = fs::create_dir_all(&log_dir).await {
        emit_error(
            &app,
            &session_id,
            &request.task_id,
            format!("无法创建日志目录：{error}"),
            None,
        );
        return;
    }
    let working_path = log_dir.join(format!("{}-进行中.log", base_name));
    let mut log_file = match OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&working_path)
        .await
    {
        Ok(file) => file,
        Err(error) => {
            emit_error(
                &app,
                &session_id,
                &request.task_id,
                format!("无法创建日志文件：{error}"),
                None,
            );
            return;
        }
    };
    let (iperf_path, _) = resolve_iperf_path(&app, &request.iperf_path);
    let (program, args) = command_for(&request, iperf_path.to_string_lossy().as_ref());
    let header = format!(
        "测试时间: {}\n客户端IP: {}\n服务端IP: {}\n测试模式: {}\n测试项目: {}\n执行命令: {} {}\n\n",
        Local::now().format("%Y-%m-%d %H:%M:%S"),
        local_ip,
        request.server_ip,
        request.mode,
        task_name,
        program,
        args.join(" ")
    );
    let _ = log_file.write_all(header.as_bytes()).await;
    emit_log(
        &app,
        &session_id,
        &request.task_id,
        "INFO",
        format!("执行：{} {}", program, args.join(" ")),
    );

    let mut command = Command::new(&program);
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
            let message = if error.kind() == std::io::ErrorKind::NotFound {
                format!("未找到 {program}。请安装 iperf3，或在配置中填写可执行文件完整路径。")
            } else {
                format!("启动测试进程失败：{error}")
            };
            let _ = log_file
                .write_all(format!("[ERROR] {message}\n").as_bytes())
                .await;
            let final_path = finish_log(&working_path, &base_name, false).await;
            emit_error(
                &app,
                &session_id,
                &request.task_id,
                message,
                Some(final_path),
            );
            return;
        }
    };
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
    let mut exit_message = None;

    loop {
        tokio::select! {
            status = child.wait() => {
                match status {
                    Ok(value) if value.success() => final_success = true,
                    Ok(value) => exit_message = Some(format!("测试进程退出，状态码：{}", value.code().map(|v| v.to_string()).unwrap_or_else(|| "unknown".into()))),
                    Err(error) => exit_message = Some(format!("等待测试进程失败：{error}")),
                }
                break;
            }
            line = rx.recv() => {
                if let Some((line, is_error)) = line {
                    let level = if is_error { "WARN" } else { "INFO" };
                    let _ = log_file.write_all(format!("[{}] {}\n", level, line).as_bytes()).await;
                    emit_log(&app, &session_id, &request.task_id, level, line.clone());
                    if let Some(metric) = parse_metric(&line) { emit_metric(&app, &session_id, &request.task_id, metric); }
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

    let outcome = if final_success { "完成" } else { "未完成" };
    let _ = log_file
        .write_all(format!("\n测试结果: {outcome}\n").as_bytes())
        .await;
    let _ = log_file.flush().await;
    drop(log_file);
    let final_path = finish_log(&working_path, &base_name, final_success).await;
    if final_stopped {
        emit_complete(&app, &session_id, &request.task_id, "stopped", final_path);
    } else if final_success {
        emit_complete(&app, &session_id, &request.task_id, "success", final_path);
    } else {
        emit_error(
            &app,
            &session_id,
            &request.task_id,
            exit_message.unwrap_or_else(|| "测试失败".into()),
            Some(final_path),
        );
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

fn command_for(request: &TestRequest, resolved_iperf: &str) -> (String, Vec<String>) {
    if request.task_id == "ping" {
        let args = if cfg!(windows) {
            vec!["-n".into(), "4".into(), request.server_ip.clone()]
        } else {
            vec!["-c".into(), "4".into(), request.server_ip.clone()]
        };
        return ("ping".into(), args);
    }
    let mut args = if request.mode == "server" || request.task_id == "server" {
        vec![
            "-s".into(),
            "-p".into(),
            request.port.to_string(),
            "-i".into(),
            request.interval.to_string(),
        ]
    } else {
        vec![
            "-c".into(),
            request.server_ip.clone(),
            "-p".into(),
            request.port.to_string(),
            "-t".into(),
            request.duration.to_string(),
            "-i".into(),
            request.interval.to_string(),
            "-f".into(),
            "m".into(),
        ]
    };
    match request.task_id.as_str() {
        "tcp-bidir" => args.push("--bidir".into()),
        "tcp-parallel" => {
            args.push("-P".into());
            args.push(request.parallel.to_string());
        }
        "tcp-reverse" => args.push("-R".into()),
        "udp-bandwidth" | "udp-loss" => {
            args.push("-u".into());
            args.push("-b".into());
            args.push(format!("{}M", request.bandwidth));
            args.push("-l".into());
            args.push(request.packet_length.to_string());
        }
        _ => {}
    }
    (resolved_iperf.to_string(), args)
}

fn parse_metric(line: &str) -> Option<MetricPoint> {
    let bandwidth = Regex::new(r"(?i)(\d+(?:\.\d+)?)-(\d+(?:\.\d+)?)\s+sec\s+(\d+(?:\.\d+)?)\s+([KMG]?Bytes)\s+(\d+(?:\.\d+)?)\s+([KMG]?bits/sec)(?:\s+(\d+))?").ok()?;
    if let Some(c) = bandwidth.captures(line) {
        let second = c.get(2)?.as_str().parse::<f64>().ok()?.round() as i64;
        let transfer = unit_to_mega(c.get(3)?.as_str().parse().ok()?, c.get(4)?.as_str());
        let speed = unit_to_mega(c.get(5)?.as_str().parse().ok()?, c.get(6)?.as_str());
        let mut metric = MetricPoint {
            second,
            bandwidth_mbps: speed,
            transfer_mb: transfer,
            retransmits: c.get(7).and_then(|v| v.as_str().parse().ok()).unwrap_or(0),
            ..Default::default()
        };
        if let Ok(udp) = Regex::new(r"(\d+(?:\.\d+)?)\s+ms\s+\d+/\d+\s+\((\d+(?:\.\d+)?)%\)") {
            if let Some(u) = udp.captures(line) {
                metric.jitter_ms = u[1].parse().unwrap_or(0.0);
                metric.loss_percent = u[2].parse().unwrap_or(0.0);
            }
        }
        return Some(metric);
    }
    let ping = Regex::new(r"(?i)(?:time|时间)[=<＝]\s*(\d+(?:\.\d+)?)\s*ms").ok()?;
    ping.captures(line).map(|c| MetricPoint {
        jitter_ms: c[1].parse().unwrap_or(0.0),
        ..Default::default()
    })
}

fn unit_to_mega(value: f64, unit: &str) -> f64 {
    if unit.starts_with('G') {
        value * 1000.0
    } else if unit.starts_with('K') {
        value / 1000.0
    } else {
        value
    }
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
        "server" => "iperf3服务端",
        _ => "网络测试",
    }
}
async fn finish_log(working: &std::path::Path, base: &str, success: bool) -> String {
    let parent = working
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let path = parent.join(format!(
        "{}-{}.log",
        base,
        if success { "完成" } else { "未完成" }
    ));
    let _ = fs::rename(working, &path).await;
    path.to_string_lossy().to_string()
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
    use super::parse_metric;

    #[test]
    fn parses_tcp_interval() {
        let metric =
            parse_metric("[  5]   0.00-1.00   sec   112 MBytes   941 Mbits/sec  3").unwrap();
        assert_eq!(metric.second, 1);
        assert_eq!(metric.bandwidth_mbps, 941.0);
        assert_eq!(metric.transfer_mb, 112.0);
        assert_eq!(metric.retransmits, 3);
    }

    #[test]
    fn parses_udp_interval() {
        let metric = parse_metric(
            "[  5]   0.00-1.00   sec  11.9 MBytes  100 Mbits/sec  0.123 ms  2/1000 (0.2%)",
        )
        .unwrap();
        assert_eq!(metric.jitter_ms, 0.123);
        assert_eq!(metric.loss_percent, 0.2);
    }

    #[test]
    fn parses_ping_latency() {
        let metric = parse_metric("Reply from 192.168.1.1: bytes=32 time=2.5ms TTL=64").unwrap();
        assert_eq!(metric.jitter_ms, 2.5);
    }
}
