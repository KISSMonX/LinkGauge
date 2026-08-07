use crate::client::{self, ClientTaskResult};
use crate::models::{ClientLogAppend, MetricPoint, ServerRuntimeStatus, TestEvent, TestRequest};
use crate::ping;
use crate::server;
use chrono::{Local, TimeZone};
use riperf3::Termination;
use std::{
    collections::{HashMap, HashSet},
    fs::File as StdFile,
    io::Write as StdWrite,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, RwLock,
    },
};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::{
    fs::{self, OpenOptions},
    sync::{watch, Mutex as AsyncMutex},
    time::{sleep, Duration},
};
use uuid::Uuid;

/// 按界面语言选择消息文案（locale 为空时默认中文）
pub(crate) fn tr<'a>(locale: &str, zh: &'a str, en: &'a str) -> &'a str {
    if locale == "en" {
        en
    } else {
        zh
    }
}

/// 按界面语言选择格式化模板（模板必须是字面量，format! 才能编译期检查）
#[macro_export]
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
    /// 当前服务端会话（单例）：重复启动服务端时拒绝。Windows 的 SO_REUSEADDR
    /// 允许同一端口双绑，双会话会造成心跳日志冲突、客户端连接随机分发
    pub(crate) server_session: Arc<AsyncMutex<Option<ServerRuntimeStatus>>>,
    /// 当前客户端队列会话（单例）：队列由后端串行驱动，避免多个窗口各自推进。
    pub(crate) client_queue_session: Arc<AsyncMutex<Option<String>>>,
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

/// 查询后端真实服务端状态。窗口刷新、分离窗口销毁或旧同步包都可能让前端丢失
/// serverRunning；不能再靠前端副本猜测服务是否仍在监听。
#[tauri::command]
pub async fn get_server_status(
    state: State<'_, AppState>,
) -> Result<Option<ServerRuntimeStatus>, String> {
    let mut server_guard = state.server_session.lock().await;
    let mut session_guard = state.sessions.lock().await;
    let Some(existing) = server_guard.clone() else {
        return Ok(None);
    };
    let active = matches!(
        session_guard.get(&existing.session_id),
        Some(SessionSignal::Engine(tx)) if !tx.is_closed()
    );
    if active {
        return Ok(Some(existing));
    }
    session_guard.remove(&existing.session_id);
    *server_guard = None;
    Ok(None)
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
pub async fn start_test<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    request: TestRequest,
) -> Result<String, String> {
    crate::validation::validate(&request)?;
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
            ping::run_ping(
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
        // 服务端单例与 sessions 在同一临界区内核对：标记存在但会话表已无该 ID
        // 说明上次异常清理留下了陈旧状态，可安全自愈；真实活动会话仍拒绝重复启动。
        // 锁顺序固定为 server_session → sessions，清理路径保持一致，避免交叉死锁。
        {
            let mut server_guard = state.server_session.lock().await;
            let mut session_guard = state.sessions.lock().await;
            if let Some(existing) = server_guard.as_ref() {
                let active = matches!(
                    session_guard.get(&existing.session_id),
                    Some(SessionSignal::Engine(tx)) if !tx.is_closed()
                );
                if active {
                    return Err(tr(
                        &request.locale,
                        "服务端已在运行，请先停止当前服务端",
                        "The server is already running. Stop it before starting again.",
                    )
                    .to_string());
                }
                session_guard.remove(&existing.session_id);
                *server_guard = None;
            }
            session_guard.insert(session_id.clone(), SessionSignal::Engine(tx));
            *server_guard = Some(ServerRuntimeStatus {
                session_id: session_id.clone(),
                bind_ip: request.bind_ip.clone(),
                port: request.port,
                interval: request.interval,
            });
        }
        let server_session = state.server_session.clone();
        tauri::async_runtime::spawn(async move {
            server::run_engine_server(app, spawned_id.clone(), request, rx, locale_handle).await;
            // 先按 ID 清除单例标记再移除会话；若期间已有新服务端启动，旧任务
            // 绝不能把新会话的标记清空。
            {
                let mut server_guard = server_session.lock().await;
                if server_guard
                    .as_ref()
                    .is_some_and(|server| server.session_id == spawned_id)
                {
                    *server_guard = None;
                }
            }
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
            client::run_engine_client(app, spawned_id.clone(), request, rx, locale_handle).await;
            sessions.lock().await.remove(&spawned_id);
        });
    }
    Ok(session_id)
}

/// 后端串行执行完整客户端队列。队列推进不再依赖某个 WebView 的 JS 定时器，
/// 因而窗口分离、销毁或后台节流都不会让下一项永远无法启动。
#[tauri::command]
pub async fn start_test_queue<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    requests: Vec<TestRequest>,
) -> Result<String, String> {
    let locale = current_locale(&state.locale);
    if requests.is_empty() {
        return Err(tr(
            &locale,
            "测试队列不能为空",
            "The test queue cannot be empty",
        )
        .into());
    }
    for request in &requests {
        if request.mode == "server" || request.task_id == "server" {
            return Err(tr(
                &locale,
                "客户端队列不能包含服务端任务",
                "A client queue cannot contain a server task",
            )
            .into());
        }
        crate::validation::validate(request)?;
    }

    let session_id = Uuid::new_v4().to_string();
    let (tx, rx) = watch::channel(None);
    {
        let mut queue_guard = state.client_queue_session.lock().await;
        let mut sessions_guard = state.sessions.lock().await;
        if let Some(existing) = queue_guard.as_ref() {
            let active = matches!(
                sessions_guard.get(existing),
                Some(SessionSignal::Engine(signal)) if !signal.is_closed()
            );
            if active {
                return Err(tr(
                    &locale,
                    "客户端测试队列已在运行，请先停止当前测试",
                    "A client test queue is already running. Stop it before starting again.",
                )
                .to_string());
            }
            sessions_guard.remove(existing);
            *queue_guard = None;
        }
        sessions_guard.insert(session_id.clone(), SessionSignal::Engine(tx));
        *queue_guard = Some(session_id.clone());
    }

    let spawned_id = session_id.clone();
    let sessions = state.sessions.clone();
    let queue_session = state.client_queue_session.clone();
    let child_pids = state.child_pids.clone();
    let locale_handle = state.locale.clone();
    tauri::async_runtime::spawn(async move {
        let mut queue_status = "success";
        let mut item_results = Vec::new();
        for request in requests {
            if rx.borrow().is_some() {
                queue_status = "stopped";
                break;
            }
            let task_id = request.task_id.clone();
            emit_task_start(&app, &spawned_id, &task_id);
            let result = if request.task_id == "ping" {
                let cancelled = Arc::new(AtomicBool::new(false));
                let forward_cancelled = cancelled.clone();
                let mut forward_rx = rx.clone();
                let forward = tokio::spawn(async move {
                    if forward_rx.changed().await.is_ok() && forward_rx.borrow().is_some() {
                        forward_cancelled.store(true, Ordering::SeqCst);
                    }
                });
                let result = ping::run_ping(
                    app.clone(),
                    spawned_id.clone(),
                    request,
                    cancelled,
                    child_pids.clone(),
                    locale_handle.clone(),
                )
                .await;
                forward.abort();
                result
            } else {
                client::run_engine_client(
                    app.clone(),
                    spawned_id.clone(),
                    request,
                    rx.clone(),
                    locale_handle.clone(),
                )
                .await
            };
            item_results.push((task_id, result));
            if result == ClientTaskResult::Fatal {
                queue_status = "failed";
                break;
            }
            if result == ClientTaskResult::Stopped {
                queue_status = "stopped";
                break;
            }
        }
        emit_queue_complete(&app, &spawned_id, queue_status, &item_results);
        {
            let mut queue_guard = queue_session.lock().await;
            if queue_guard.as_deref() == Some(spawned_id.as_str()) {
                *queue_guard = None;
            }
        }
        sessions.lock().await.remove(&spawned_id);
    });
    Ok(session_id)
}

#[tauri::command]
pub async fn stop_test(state: State<'_, AppState>, session_id: String) -> Result<(), String> {
    let sessions = state.sessions.clone();
    let mut already_ended = false;
    {
        let mut map = sessions.lock().await;
        match map.get(&session_id) {
            Some(SessionSignal::Ping(flag)) => flag.store(true, Ordering::SeqCst),
            Some(SessionSignal::Engine(tx)) if tx.is_closed() => {
                // 任务异常退出时 spawn 清理代码可能未执行；关闭的通道已不代表活动会话。
                already_ended = true;
            }
            Some(SessionSignal::Engine(tx)) => {
                let _ = tx.send(Some("用户手动停止".into()));
            }
            None => already_ended = true,
        }
        if already_ended {
            map.remove(&session_id);
        }
    }
    if already_ended {
        let mut server_guard = state.server_session.lock().await;
        if server_guard
            .as_ref()
            .is_some_and(|server| server.session_id == session_id)
        {
            *server_guard = None;
        }
        drop(server_guard);
        let mut queue_guard = state.client_queue_session.lock().await;
        if queue_guard.as_deref() == Some(session_id.as_str()) {
            *queue_guard = None;
        }
        return Ok(());
    }
    // stop_test 的完成语义必须是「任务已退出」，不能只是「信号已发送」；否则前端
    // 会先显示已停止并允许重启，而后端单例仍在清理，随即误报“服务端已在运行”。
    for _ in 0..100 {
        if !sessions.lock().await.contains_key(&session_id) {
            return Ok(());
        }
        sleep(Duration::from_millis(50)).await;
    }
    Err(tr(
        &current_locale(&state.locale),
        "停止请求已发送，但任务未在 5 秒内退出",
        "The stop request was sent, but the task did not exit within 5 seconds.",
    )
    .to_string())
}

/// 用系统文件管理器打开测试日志目录，返回目录路径
#[tauri::command]
pub async fn open_log_dir<R: tauri::Runtime>(app: AppHandle<R>) -> Result<String, String> {
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

/// 日志文件句柄：回调（同步）与主任务（异步）共用，加锁串行写入
pub(crate) struct TestLog {
    pub(crate) file: Arc<Mutex<StdFile>>,
    pub(crate) working_path: std::path::PathBuf,
    pub(crate) base_name: String,
    /// 客户端运行日志：一轮队列的所有测试项共用并持续追加，finish 时不重命名；
    /// false = 服务端单会话文件，完成时重命名为 -completed/-incomplete
    pub(crate) shared: bool,
}
pub(crate) type SessionLog = Arc<TestLog>;




// ---------------------------------------------------------------------------
// 日志与结果处理
// ---------------------------------------------------------------------------

/// 创建测试日志文件并写入文件头，失败时发出错误事件
pub(crate) async fn setup_log<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: &str,
    request: &TestRequest,
    local_ip: &str,
    task_name: &str,
) -> Option<SessionLog> {
    let stamp = Local::now().format("%Y%m%d%H%M%S").to_string();
    // 服务端与客户端日志分开记录。客户端不再按测试项分文件：一轮队列的所有
    // 测试项（同一 runId）写入同一个运行日志 客户端-{本机IP}-{对端IP}-{运行时间}，
    // 前缀随界面语言（客户端 / Client）；无 runId 时退化为本次会话时间戳。
    // 服务端保持单会话文件 服务端-{本机IP}-{端口}-{时间}
    let (shared, base_name) = if request.mode == "server" || request.task_id == "server" {
        (
            false,
            format!(
                "{}-{}-{}-{}",
                tr(&request.locale, "服务端", "Server"),
                safe_name(local_ip),
                request.port,
                stamp
            ),
        )
    } else {
        let run_stamp = if request.run_id == 0 {
            stamp.clone()
        } else {
            Local
                .timestamp_millis_opt(request.run_id)
                .single()
                .map(|t| t.format("%Y%m%d%H%M%S").to_string())
                .unwrap_or_else(|| stamp.clone())
        };
        (
            true,
            format!(
                "{}-{}-{}-{}",
                tr(&request.locale, "客户端", "Client"),
                safe_name(local_ip),
                safe_name(&request.server_ip),
                run_stamp
            ),
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
            false,
        );
        return None;
    }
    // 共享运行日志直接以最终文件名存在（无「进行中」后缀，也不重命名）；
    // 服务端保留 进行中/in progress 工作名，完成时由 finish_log 重命名
    let working_path = if shared {
        log_dir.join(format!("{base_name}.log"))
    } else {
        log_dir.join(format!(
            "{}-{}.log",
            base_name,
            tr(&request.locale, "进行中", "in progress")
        ))
    };
    let file = match OpenOptions::new()
        .create(true)
        .append(shared) // 共享日志跨测试项追加；服务端会话文件从零开始
        .write(!shared)
        .truncate(!shared)
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
                false,
            );
            return None;
        }
    };
    let header = tr_format!(
        &request.locale,
        "测试时间: {}\n客户端IP: {}\n服务端IP: {}\n测试模式: {}\n测试项目: {}",
        "Test time: {}\nClient IP: {}\nServer IP: {}\nMode: {}\nTest item: {}",
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
        shared,
    });
    append_log(&log, &header);
    Some(log)
}

/// 同步追加一行日志（实时回调与主任务共用）。统一补换行：日志文件与界面日志
/// 一样逐行呈现（此前只写内容不换行，多行输出在文件里粘成一行）
pub(crate) fn append_log(log: &SessionLog, line: &str) {
    if let Ok(mut file) = log.file.lock() {
        let _ = file.write_all(line.as_bytes());
        let _ = file.write_all(b"\n");
    }
}

/// 客户端运行日志的路径推导（与 setup_log 的客户端分支一致）：按 runId 时间戳
/// 定位，前缀随界面语言（客户端 / Client）。运行中切换界面语言时前缀可能与
/// 创建时不同，按时间戳在日志目录中找回既有文件，避免超时补写落到新文件
pub(crate) fn client_run_log_path<R: tauri::Runtime>(
    app: &AppHandle<R>,
    run_id: i64,
    local_ip: &str,
    server_ip: &str,
    locale: &str,
) -> Option<std::path::PathBuf> {
    if run_id == 0 {
        return None;
    }
    let run_stamp = Local
        .timestamp_millis_opt(run_id)
        .single()?
        .format("%Y%m%d%H%M%S")
        .to_string();
    let log_dir = app
        .path()
        .app_log_dir()
        .unwrap_or_else(|_| std::env::temp_dir().join("linkgauge"))
        .join("tests");
    let name = format!(
        "{}-{}-{}-{}.log",
        tr(locale, "客户端", "Client"),
        safe_name(local_ip),
        safe_name(server_ip),
        run_stamp
    );
    let path = log_dir.join(&name);
    if path.exists() {
        return Some(path);
    }
    let suffix = format!("-{run_stamp}.log");
    std::fs::read_dir(&log_dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                n.ends_with(&suffix) && (n.starts_with("客户端-") || n.starts_with("Client-"))
            })
        })
        .or(Some(path))
}

/// 前端生成的队列级日志补写客户端运行日志文件：看门狗超时 / 首事件探针失败 /
/// 驱动接管等前端日志不在后端事件流里，经此落入运行日志；写入失败静默跳过
#[tauri::command]
pub async fn append_client_log<R: tauri::Runtime>(app: AppHandle<R>, request: ClientLogAppend) {
    let Some(path) = client_run_log_path(
        &app,
        request.run_id,
        &request.local_ip,
        &request.server_ip,
        &request.locale,
    ) else {
        return;
    };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = file.write_all(format!("[{}] {}\n", request.level, request.message).as_bytes());
    }
}

/// 客户端连不上服务端（未启动 / 端口错误 / 主机名解析失败 / 连接超时）——环境性
/// 问题，队列里剩余引擎项必然同样失败。前端收到 fatal 事件后中止整个队列。
/// 注意不能按 Io 错误一锅端：如 MPTCP 在不支持的平台上也是 Io(Unsupported)，
/// 那种逐项失败（如 tcp-mptcp）应继续队列而不是中止。
pub(crate) fn is_server_unreachable(error: &riperf3::RiperfError) -> bool {
    match error {
        riperf3::RiperfError::Io(e) => matches!(
            e.kind(),
            std::io::ErrorKind::ConnectionRefused
                | std::io::ErrorKind::NotFound
                | std::io::ErrorKind::TimedOut
        ),
        riperf3::RiperfError::ConnectionTimeout => true,
        _ => false,
    }
}

/// 客户端结束时按 RunOutcome 归类结果：成功 / 手动停止 / 失败
pub(crate) async fn finish_engine<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: &str,
    task_id: &str,
    log: &SessionLog,
    request: &TestRequest,
    locale: &str,
    outcome: Result<riperf3::RunOutcome, riperf3::RiperfError>,
) -> ClientTaskResult {
    match outcome {
        Err(error) => {
            // 服务端不可达（未启动 / 端口错 / DNS 失败 / 超时）单独归类：给出可操作
            // 的排查文案并标记 fatal，前端收到后中止整个队列
            let (message, fatal) = if is_server_unreachable(&error) {
                (
                    tr_format!(
                        locale,
                        "无法连接服务端：{}。请确认服务端已启动、IP 与端口正确。",
                        "Cannot reach the server: {}. Verify the server is running and the IP/port are correct.",
                        error
                    ),
                    true,
                )
            } else if matches!(
                &error,
                riperf3::RiperfError::Io(e) if e.kind() == std::io::ErrorKind::Unsupported
            ) {
                // 平台不支持（如 Windows/macOS 内核无 IPPROTO_MPTCP，socket 创建报
                // WSAEOPNOTSUPP）：给出明确文案，而非引擎的「无法连接服务端」误导
                // 信息；逐项失败继续队列（不中止）
                (
                    tr_format!(
                        locale,
                        "当前平台不支持该测试项（协议未配置或不可用）：{}",
                        "This test item is not supported on the current platform (protocol not configured or unavailable): {}",
                        error
                    ),
                    false,
                )
            } else {
                // 两类可操作的失败给出具体排查方向，其余沿用引擎原文
                let message = match &error {
                    riperf3::RiperfError::ServerBusy => tr(
                        locale,
                        "服务端忙：iperf3 一次只服务一个测试。请确认没有其他客户端正在占用该服务端。",
                        "Server busy: iperf3 serves one test at a time. Check that no other client is using that server.",
                    )
                    .to_string(),
                    riperf3::RiperfError::AccessDenied => tr(
                        locale,
                        "认证被服务端拒绝：请核对用户名、密码，以及 RSA 公钥是否与服务端 --authorized-users-path 中的条目匹配；若对端 iperf3 低于 3.17，需勾选「PKCS#1 填充」。",
                        "Authentication rejected: verify the username, password, and that the RSA public key matches an entry in the server's --authorized-users-path. Tick \"PKCS#1 padding\" if the peer iperf3 is older than 3.17.",
                    )
                    .to_string(),
                    _ => tr_format!(locale, "测试失败：{}", "Test failed: {}", error),
                };
                (message, false)
            };
            fail_engine(app, session_id, task_id, log, locale, &message, fatal).await;
            if fatal {
                ClientTaskResult::Fatal
            } else {
                ClientTaskResult::Failed
            }
        }
        Ok(outcome) => {
            let report = &outcome.report;
            append_log(
                log,
                &format!(
                    "[INFO] {}",
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
                let block = format!("[INFO] {header}\n{text}");
                append_log(log, &block);
                emit_log(app, session_id, task_id, "INFO", block);
            }
            // 汇总终点即实测结束秒（按量模式 / 预热下与名义 duration 不同，须取实测值）
            let measured_end = report
                .end
                .sum_sent
                .as_ref()
                .or(report.end.sum_received.as_ref())
                .map(|s| s.end.ceil().max(1.0) as i64)
                .unwrap_or(request.duration.max(1) as i64);
            emit_final_summary(app, session_id, task_id, report, measured_end);
            match outcome.termination {
                Termination::Completed => {
                    append_log(log, tr(locale, "测试结果: 完成", "Result: completed"));
                    finish_ok(app, session_id, task_id, log, "success").await;
                    ClientTaskResult::Success
                }
                Termination::Interrupted => {
                    append_log(log, tr(locale, "测试结果: 手动停止", "Result: manual stop"));
                    finish_ok(app, session_id, task_id, log, "stopped").await;
                    ClientTaskResult::Stopped
                }
                Termination::ServerTerminated => {
                    let message = tr(
                        locale,
                        "服务端主动终止了测试",
                        "The server terminated the test",
                    )
                    .to_string();
                    fail_engine(app, session_id, task_id, log, locale, &message, false).await;
                    ClientTaskResult::Failed
                }
                Termination::ServerError(msg) => {
                    let message = tr_format!(
                        locale,
                        "服务端返回错误：{}",
                        "Server returned an error: {}",
                        msg
                    );
                    fail_engine(app, session_id, task_id, log, locale, &message, false).await;
                    ClientTaskResult::Failed
                }
                other => {
                    let message = tr_format!(
                        locale,
                        "测试异常结束：{:?}",
                        "Test ended abnormally: {:?}",
                        other
                    );
                    fail_engine(app, session_id, task_id, log, locale, &message, false).await;
                    ClientTaskResult::Failed
                }
            }
        }
    }
}

/// 追加最终汇总（发送/接收方向的传输量与平均带宽，UDP 附抖动丢包）
/// 最终汇总文本（发送/接收方向的传输量与平均带宽，UDP 附抖动丢包）：逐行
/// [INFO] 前缀，测试项日志与客户端运行汇总文件共用
pub(crate) fn engine_summary_lines(report: &riperf3::Report, locale: &str) -> String {
    let mut out = String::new();
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
        out.push_str(&format!(
            "[INFO] {line}
"
        ));
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
        out.push_str(&format!(
            "[INFO] {line}
"
        ));
    }
    out
}

pub(crate) fn append_engine_summary(log: &SessionLog, report: &riperf3::Report, locale: &str) {
    let lines = engine_summary_lines(report, locale);
    if !lines.is_empty() {
        append_log(log, lines.trim_end());
    }
}

/// 最终汇总与区间采样语义不同：它是全程平均带宽与总传输量，不能追加到实时
/// 曲线，否则最后会在同一时间坐标产生明显竖直突变。
pub(crate) fn emit_final_summary<R: tauri::Runtime>(
    app: &AppHandle<R>,
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
        let _ = app.emit(
            "test-event",
            TestEvent {
                session_id: session_id.into(),
                task_id: task_id.into(),
                event_type: "summary".into(),
                status: None,
                level: None,
                message: None,
                metric: Some(MetricPoint {
                    second,
                    bandwidth_mbps: side.bits_per_second / 1_000_000.0,
                    transfer_mb: side.bytes as f64 / 1_000_000.0,
                    jitter_ms: side.jitter_ms.unwrap_or(0.0),
                    loss_percent: side.lost_percent.unwrap_or(0.0),
                    retransmits: side.retransmits.unwrap_or(0).max(0) as u64,
                }),
                log_path: None,
                fatal: None,
            },
        );
    }
}

/// 逐秒指标行的日志格式（近似 iperf3 文本输出）
pub(crate) fn format_interval_line(
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

pub(crate) async fn finish_ok<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: &str,
    task_id: &str,
    log: &SessionLog,
    status: &str,
) {
    let success = status == "success";
    let log_path = finish_log(log, success).await;
    emit_complete(app, session_id, task_id, status, log_path);
}

pub(crate) async fn fail_engine<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: &str,
    task_id: &str,
    log: &SessionLog,
    locale: &str,
    message: &str,
    fatal: bool,
) {
    append_log(log, &format!("[ERROR] {message}"));
    let log_path = finish_log(log, false).await;
    let full = format!(
        "{message}{}",
        tr_format!(locale, "详细日志：{}", "Detailed log: {}", log_path)
    );
    emit_error(app, session_id, task_id, full, Some(log_path), fatal);
}

/// 关闭日志文件并按完成状态重命名（完成/未完成/手动停止均保留日志）
pub(crate) async fn finish_log(log: &SessionLog, success: bool) -> String {
    if let Ok(mut file) = log.file.lock() {
        let _ = file.flush();
    }
    if log.shared {
        // 客户端运行日志：整轮队列持续追加，不重命名
        return log.working_path.to_string_lossy().to_string();
    }
    let final_path = log.working_path.with_file_name(format!(
        "{}-{}.log",
        log.base_name,
        if success { "completed" } else { "incomplete" }
    ));
    let _ = fs::rename(&log.working_path, &final_path).await;
    final_path.to_string_lossy().to_string()
}

pub(crate) fn safe_name(value: &str) -> String {
    value
        .chars()
        .map(|c| if "<>:\"/\\|?*".contains(c) { '_' } else { c })
        .collect()
}

/// 任务显示名：随界面语言输出，日志文件名（经 safe_name 净化）与日志头部共用——
/// 英文模式下产出英文文件名（如 Client-…-TCP One-way Bandwidth-…）
pub(crate) fn task_label(locale: &str, id: &str) -> &'static str {
    if locale == "en" {
        match id {
            "ping" => "Ping Connectivity",
            "tcp-single" => "TCP One-way Bandwidth",
            "tcp-bidir" => "TCP Bidirectional Bandwidth",
            "tcp-parallel" => "TCP Parallel Streams",
            "udp-bandwidth" => "UDP Bandwidth",
            "udp-loss" => "UDP Jitter Loss",
            "tcp-reverse" => "TCP Reverse Test",
            "stress" => "Sustained Stress Test",
            "tcp-bytes" => "TCP Byte-Limited Test",
            "udp-bytes" => "UDP Byte-Limited Test",
            "tcp-blocks" => "TCP Block-Limited Test",
            "tcp-mptcp" => "TCP MPTCP Test",
            "udp-df" => "UDP No-Fragment Test",
            "server" => "riperf3 server",
            _ => "Network test",
        }
    } else {
        match id {
            "ping" => "Ping连通性测试",
            "tcp-single" => "TCP单向带宽",
            "tcp-bidir" => "TCP双向带宽",
            "tcp-parallel" => "TCP多并发流",
            "udp-bandwidth" => "UDP带宽",
            "udp-loss" => "UDP抖动丢包",
            "tcp-reverse" => "TCP反向测试",
            "stress" => "持续压力测试",
            "tcp-bytes" => "TCP按量传输测试",
            "udp-bytes" => "UDP按量传输测试",
            "tcp-blocks" => "TCP按块传输测试",
            "tcp-mptcp" => "TCP多路径测试",
            "udp-df" => "UDP无分片测试",
            "server" => "riperf3服务端",
            _ => "网络测试",
        }
    }
}

pub(crate) fn emit_log<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session: &str,
    task: &str,
    level: &str,
    message: String,
) {
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
            fatal: None,
        },
    );
}
pub(crate) fn emit_task_start<R: tauri::Runtime>(app: &AppHandle<R>, session: &str, task: &str) {
    let _ = app.emit(
        "test-event",
        TestEvent {
            session_id: session.into(),
            task_id: task.into(),
            event_type: "start".into(),
            status: Some("running".into()),
            level: None,
            message: None,
            metric: None,
            log_path: None,
            fatal: None,
        },
    );
}
pub(crate) fn emit_queue_complete<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session: &str,
    status: &str,
    item_results: &[(String, ClientTaskResult)],
) {
    let items: Vec<serde_json::Value> = item_results
        .iter()
        .map(|(task_id, result)| {
            serde_json::json!({
                "taskId": task_id,
                "status": match result {
                    ClientTaskResult::Success => "success",
                    ClientTaskResult::Failed | ClientTaskResult::Fatal => "failed",
                    ClientTaskResult::Stopped => "stopped",
                }
            })
        })
        .collect();
    let _ = app.emit(
        "test-event",
        TestEvent {
            session_id: session.into(),
            task_id: String::new(),
            event_type: "queue-complete".into(),
            status: Some(status.into()),
            level: None,
            // 队列结束时携带最终逐项状态，供所有窗口一次性校准；否则漏掉某个
            // complete 的旧窗口会把 running 状态通过停止同步覆盖回其他窗口。
            message: Some(serde_json::json!({ "items": items }).to_string()),
            metric: None,
            log_path: None,
            fatal: None,
        },
    );
}
pub(crate) fn emit_metric<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session: &str,
    task: &str,
    metric: MetricPoint,
) {
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
            fatal: None,
        },
    );
}
pub(crate) fn emit_complete<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session: &str,
    task: &str,
    status: &str,
    path: String,
) {
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
            fatal: None,
        },
    );
}
pub(crate) fn emit_error<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session: &str,
    task: &str,
    message: String,
    path: Option<String>,
    fatal: bool,
) {
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
            fatal: Some(fatal),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::{
        append_log, is_server_unreachable,
        TestLog,
    };
    use crate::client::{client_params_for, client_task_timeout};
    use crate::models::TestRequest;
    use crate::ping::parse_ping_metric;
    use riperf3::TransportProtocol;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// 日志文件逐行换行：append_log 统一补 \n，多行内容与界面日志一样分行呈现
    #[test]
    fn append_log_separates_lines_with_newline() {
        let dir = std::env::temp_dir().join(format!("linkgauge-log-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.log");
        let file = std::fs::File::create(&path).unwrap();
        let log = Arc::new(TestLog {
            file: Arc::new(Mutex::new(file)),
            working_path: path.clone(),
            base_name: "test".into(),
            shared: false,
        });
        append_log(&log, "[INFO] 第一行");
        append_log(&log, "[INFO] 第二行");
        append_log(&log, "多行内容\n内含换行");
        drop(log);
        let content = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            content,
            "[INFO] 第一行\n[INFO] 第二行\n多行内容\n内含换行\n"
        );
    }

    #[test]
    fn ping_metrics_keep_sample_order_for_charting() {
        let first = parse_ping_metric("Reply from 127.0.0.1: time<1ms", 1).unwrap();
        let second = parse_ping_metric("来自 127.0.0.1 的回复: 时间=2ms", 2).unwrap();
        assert_eq!(first.second, 1);
        assert_eq!(second.second, 2);
        assert_eq!(first.jitter_ms, 1.0);
        assert_eq!(second.jitter_ms, 2.0);
    }

    #[test]
    fn backend_timeout_covers_time_bytes_and_blocks_modes() {
        let mut request = request("tcp-single", "client", "tcp");
        request.duration = 30;
        assert_eq!(client_task_timeout(&request), Duration::from_secs(60));

        request.task_id = "tcp-bytes".into();
        request.transfer_amount = 1_000;
        request.bandwidth = 100;
        assert_eq!(client_task_timeout(&request), Duration::from_secs(95));

        request.task_id = "tcp-blocks".into();
        request.transfer_amount = 100_000;
        request.packet_length = 1_000;
        assert_eq!(client_task_timeout(&request), Duration::from_secs(30));
    }

    fn request(task_id: &str, mode: &str, protocol: &str) -> TestRequest {
        TestRequest {
            task_id: task_id.into(),
            mode: mode.into(),
            run_id: 0,
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
        assert!(crate::validation::validate(&req).is_err());
        req.server_auth_users_path = "users.csv".into();
        assert!(crate::validation::validate(&req).is_ok());
        // 未启用时缺路径不报错（保持旧行为）
        req.server_auth_enabled = false;
        req.server_auth_private_key_path.clear();
        req.server_auth_users_path.clear();
        assert!(crate::validation::validate(&req).is_ok());
        // 客户端模式不受服务端认证字段影响
        let mut client = request("tcp-single", "client", "tcp");
        client.server_auth_enabled = true;
        assert!(crate::validation::validate(&client).is_ok());
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
        assert!(crate::validation::validate(&req).is_err());
        // 预热 > 时长：拒绝
        req.omit_secs = 11;
        assert!(crate::validation::validate(&req).is_err());
        // 预热 < 时长：通过
        req.omit_secs = 9;
        assert!(crate::validation::validate(&req).is_ok());
        // 0 = 不预热：通过
        req.omit_secs = 0;
        assert!(crate::validation::validate(&req).is_ok());
    }

    #[test]
    fn window_kb_capped_at_16mb() {
        let mut req = request("tcp-single", "client", "tcp");
        req.window_kb = 16384;
        assert!(crate::validation::validate(&req).is_ok());
        req.window_kb = 16385;
        assert!(crate::validation::validate(&req).is_err());
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
            assert!(crate::validation::validate(&req).is_ok(), "ip_version={valid} 应通过");
        }
        for invalid in [1, 3, 5, 7, 255] {
            req.ip_version = invalid;
            assert!(crate::validation::validate(&req).is_err(), "ip_version={invalid} 应被拒绝");
        }
    }

    #[test]
    fn server_protection_params_validated() {
        let mut req = request("server", "server", "tcp");
        req.duration = 0;
        // 默认 0 = 不限制：全部通过
        assert!(crate::validation::validate(&req).is_ok());
        // 上限边界：86400 通过，超限拒绝
        req.server_idle_timeout = 86400;
        req.server_max_duration = 86400;
        assert!(crate::validation::validate(&req).is_ok());
        req.server_idle_timeout = 86401;
        assert!(crate::validation::validate(&req).is_err());
        req.server_idle_timeout = 0;
        req.server_max_duration = 86401;
        assert!(crate::validation::validate(&req).is_err());
        req.server_max_duration = 0;
        // 带宽上限：1_000_000 Mbps 通过，超限拒绝
        req.server_bitrate_limit_mbps = 1_000_000;
        assert!(crate::validation::validate(&req).is_ok());
        req.server_bitrate_limit_mbps = 1_000_001;
        assert!(crate::validation::validate(&req).is_err());
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
        assert!(crate::validation::validate(&req).is_err());
        req.transfer_amount = 1;
        assert!(crate::validation::validate(&req).is_ok());
        // 非法结束条件直接拒绝
        req.transfer_mode = "packets".into();
        assert!(crate::validation::validate(&req).is_err());
        // 按量模式忽略时长校验：duration 为 0 也应通过（time 模式则拒绝）
        req.transfer_mode = "time".into();
        req.duration = 0;
        assert!(crate::validation::validate(&req).is_err());
        req.transfer_mode = "bytes".into();
        req.transfer_amount = 1;
        assert!(crate::validation::validate(&req).is_ok());
        req.duration = 10;
        // 预热与按量互斥
        req.omit_secs = 1;
        assert!(crate::validation::validate(&req).is_err());
        req.transfer_mode = "time".into();
        assert!(crate::validation::validate(&req).is_ok());
        req.omit_secs = 0;
    }

    #[test]
    fn dscp_range_validated() {
        let mut req = request("tcp-single", "client", "tcp");
        req.dscp = 63;
        assert!(crate::validation::validate(&req).is_ok());
        req.dscp = 64;
        assert!(crate::validation::validate(&req).is_err());
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
        assert!(crate::validation::validate(&req).is_err());
        req.transfer_amount = 1;
        assert!(crate::validation::validate(&req).is_ok());
        // 按量项与预热互斥
        req.omit_secs = 1;
        assert!(crate::validation::validate(&req).is_err());
        // 普通项 + 全局按量模式下：按量项校验仍生效
        let mut mixed = request("tcp-single", "client", "tcp");
        mixed.transfer_amount = 2;
        assert!(crate::validation::validate(&mixed).is_ok());
    }

    #[test]
    fn server_request_without_transfer_mode_passes_validation() {
        // 回归：服务端启动请求不携带 transfer_mode（serde 缺省空串），
        // 空串必须按 time 处理，否则「启动服务」报错
        let mut req = request("server", "server", "tcp");
        req.duration = 0;
        req.transfer_mode = String::new();
        assert!(crate::validation::validate(&req).is_ok());
        // 空串归一化不放松非空非法值校验
        let mut bad = request("tcp-single", "client", "tcp");
        bad.transfer_mode = "packets".into();
        assert!(crate::validation::validate(&bad).is_err());
    }

    /// 服务端不可达分类：连接被拒 / DNS 失败 / 超时 → fatal；平台不支持（如
    /// Windows 上的 MPTCP，同样落在 Io）与认证拒绝等 → 非 fatal（队列继续）
    #[test]
    fn server_unreachable_classification() {
        use std::io::ErrorKind;
        let io = |kind| riperf3::RiperfError::Io(std::io::Error::from(kind));
        assert!(is_server_unreachable(&io(ErrorKind::ConnectionRefused)));
        assert!(is_server_unreachable(&io(ErrorKind::NotFound)));
        assert!(is_server_unreachable(&io(ErrorKind::TimedOut)));
        assert!(is_server_unreachable(
            &riperf3::RiperfError::ConnectionTimeout
        ));
        // 非 fatal：MPTCP 在 Windows 上就是 Io(Unsupported)，逐项失败应继续队列
        assert!(!is_server_unreachable(&io(ErrorKind::Unsupported)));
        assert!(!is_server_unreachable(&riperf3::RiperfError::ServerBusy));
        assert!(!is_server_unreachable(&riperf3::RiperfError::AccessDenied));
    }
}

#[cfg(test)]
mod queue_tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tauri::Listener;

    /// 基础请求（tests 模块的 request 为兄弟模块私有，此处内联构造）
    fn base_request(task_id: &str, port: u16, run_id: i64) -> TestRequest {
        TestRequest {
            task_id: task_id.into(),
            mode: "client".into(),
            run_id, // 每次运行唯一：汇总文件按 runId 区分，避免跨运行累积
            protocol: String::new(),
            server_ip: "127.0.0.1".into(),
            local_ip: "127.0.0.1".into(),
            bind_ip: String::new(),
            locale: String::new(),
            port,
            duration: 1,
            parallel: 1,
            bandwidth: 0,
            packet_length: 131072,
            udp_packet_length: 1460,
            interval: 1,
            omit_secs: 0,
            window_kb: 0,
            cport: 0,
            ip_version: 0,
            get_server_output: false,
            transfer_mode: "time".into(),
            transfer_amount: 1, // 1MB：快任务
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

    /// 服务端异常退出可能留下单例标记和已关闭的会话信号；下一次启动应自愈，
    /// 正常停止则必须等清理完成，随后可立即在同一端口重新启动。
    #[tokio::test(flavor = "multi_thread")]
    async fn server_session_recovers_stale_state_and_restarts_after_stop() {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);

        let app = tauri::test::mock_builder()
            .manage(AppState::default())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let handle = app.handle().clone();
        let stale_id = "stale-server-session".to_string();
        let (stale_tx, stale_rx) = tokio::sync::watch::channel(None);
        drop(stale_rx);
        {
            let state = app.state::<AppState>();
            state
                .sessions
                .lock()
                .await
                .insert(stale_id.clone(), SessionSignal::Engine(stale_tx));
            *state.server_session.lock().await = Some(ServerRuntimeStatus {
                session_id: stale_id.clone(),
                bind_ip: "127.0.0.1".into(),
                port,
                interval: 1,
            });
        }

        let mut request = base_request("server", port, Local::now().timestamp_millis());
        request.mode = "server".into();
        request.bind_ip = "127.0.0.1".into();
        request.duration = 0;

        let first = start_test(handle.clone(), app.state::<AppState>(), request.clone())
            .await
            .expect("已关闭的服务端会话不应阻止重新启动");
        assert_ne!(first, stale_id);
        assert!(app
            .state::<AppState>()
            .sessions
            .lock()
            .await
            .get(&stale_id)
            .is_none());

        let duplicate = start_test(handle.clone(), app.state::<AppState>(), request.clone()).await;
        assert!(duplicate.is_err(), "活动服务端仍必须拒绝重复启动");
        let status = get_server_status(app.state::<AppState>())
            .await
            .unwrap()
            .expect("活动服务端必须可供重建窗口恢复");
        assert_eq!(status.session_id, first);
        assert_eq!(status.bind_ip, "127.0.0.1");
        assert_eq!(status.port, port);

        stop_test(app.state::<AppState>(), first).await.unwrap();
        assert!(app
            .state::<AppState>()
            .server_session
            .lock()
            .await
            .is_none());

        let second = start_test(handle, app.state::<AppState>(), request)
            .await
            .expect("停止返回后应能立即重新启动服务端");
        stop_test(app.state::<AppState>(), second).await.unwrap();
    }

    /// 快任务链端到端：tcp-bytes → udp-bytes → tcp-blocks 连续毫秒级任务，
    /// 每个都必须收到 complete（复现「卡在第 N 项」——若后端事件缺失此处直接失败）
    #[tokio::test(flavor = "multi_thread")]
    async fn fast_task_chain_emits_complete_per_item() {
        // 固定端口：UDP 任务的控制与 demux 必须同端口（port 0 会错开）
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);

        // 服务端：循环 run_once 持续服务，测试结束发中断退出
        let (server_tx, server_rx) = tokio::sync::watch::channel(None);
        let server = riperf3::ServerBuilder::new()
            .port(Some(port))
            .one_off(true)
            .json_output(true)
            .emit_output(false)
            .interrupt(server_rx)
            .build()
            .unwrap();
        let bound = server.bind().await.unwrap();
        let server_task = tokio::spawn(async move {
            loop {
                if bound.run_once().await.is_err() {
                    break;
                }
            }
        });

        // mock app + test-event 捕获（app.emit 会触发 Rust 侧 listen 处理器）
        let app = tauri::test::mock_builder()
            .manage(AppState::default())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let handle = app.handle().clone();
        let events: Arc<Mutex<Vec<(String, String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let hook = events.clone();
        handle.listen("test-event", move |e| {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(e.payload()) {
                let task = value["taskId"].as_str().unwrap_or("").to_string();
                let kind = value["type"].as_str().unwrap_or("").to_string();
                let message = value["message"].as_str().unwrap_or("").to_string();
                hook.lock().unwrap().push((task, kind, message));
            }
        });

        // 复现用户队列：ping 后连续 TCP/UDP 项（含 bidir 后的下一个连接——
        // 服务端 run_once 处理反向连接返回后需回到监听，若服务端卡在旧会话，
        // 后续项的 complete 会缺失）
        // 每次运行唯一 runId：汇总文件按 runId 命名，避免与历史运行的文件混淆
        let run_id = Local::now().timestamp_millis();
        let task_ids = [
            "tcp-single",
            "tcp-bidir",
            "tcp-parallel",
            "udp-bandwidth",
            "udp-loss",
            "tcp-bytes",
            "udp-bytes",
            "tcp-blocks",
        ];
        let requests = task_ids
            .iter()
            .map(|task_id| base_request(task_id, port, run_id))
            .collect();
        let session = start_test_queue(handle.clone(), app.state::<AppState>(), requests)
            .await
            .expect("start_test_queue 应成功");
        for task_id in task_ids {
            // 等待该任务的 complete（30 秒超时）
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
            loop {
                let done = events
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|(t, ty, _)| t == task_id && ty == "complete");
                if done {
                    break;
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "{task_id} 未收到 complete（session {session}）"
                );
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
        let queue_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while !events
            .lock()
            .unwrap()
            .iter()
            .any(|(_, kind, _)| kind == "queue-complete")
        {
            assert!(
                tokio::time::Instant::now() < queue_deadline,
                "后端完成全部任务后必须发出 queue-complete"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let starts: Vec<String> = events
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, kind, _)| kind == "start")
            .map(|(task, _, _)| task.clone())
            .collect();
        assert_eq!(starts, task_ids, "后端必须严格按队列顺序启动每个测试项");
        {
            let captured = events.lock().unwrap();
            for task_id in task_ids {
                let summary = captured
                    .iter()
                    .position(|(task, kind, _)| task == task_id && kind == "summary")
                    .unwrap_or_else(|| panic!("{task_id} 必须发出独立最终汇总"));
                let complete = captured
                    .iter()
                    .position(|(task, kind, _)| task == task_id && kind == "complete")
                    .unwrap();
                assert!(
                    summary < complete,
                    "{task_id} 必须先汇总再完成，前端才能保存曲线"
                );
            }
            assert!(
                captured
                    .iter()
                    .any(|(_, kind, message)| kind == "log" && message.starts_with("第 ")),
                "输出周期指标必须同步到界面日志，不能长期停在开始测试"
            );
        }
        let final_message = events
            .lock()
            .unwrap()
            .iter()
            .find(|(_, kind, _)| kind == "queue-complete")
            .map(|(_, _, message)| message.clone())
            .unwrap();
        let final_state: serde_json::Value = serde_json::from_str(&final_message).unwrap();
        let final_items = final_state["items"].as_array().unwrap();
        assert_eq!(final_items.len(), task_ids.len());
        assert!(final_items
            .iter()
            .all(|item| item["status"].as_str() == Some("success")));

        // 客户端运行汇总文件：同一 runId 的 8 个测试项写入同一个文件，逐项有结果
        let summary_dir = app.path().app_log_dir().unwrap().join("tests");
        let run_stamp = Local
            .timestamp_millis_opt(run_id)
            .single()
            .unwrap()
            .format("%Y%m%d%H%M%S")
            .to_string();
        let summary_file = std::fs::read_dir(&summary_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| {
                let name = p.file_name().unwrap_or_default().to_string_lossy();
                name.starts_with("客户端-") && name.contains(&format!("-{run_stamp}.log"))
            })
            .expect("应生成客户端汇总文件");
        let content = std::fs::read_to_string(&summary_file).unwrap();
        assert!(
            content.contains("测试项目: TCP单向带宽"),
            "运行日志应包含测试项目行"
        );
        assert!(
            content.contains("执行：riperf3"),
            "汇总文件应包含执行过程（执行行）"
        );
        assert_eq!(
            content.matches("测试结果: 完成").count(),
            8,
            "8 个测试项都应写入完成结果"
        );
        // 前端队列级错误（看门狗超时等）经 append_client_log 补写运行日志
        append_client_log(
            handle.clone(),
            ClientLogAppend {
                run_id,
                local_ip: "127.0.0.1".into(),
                server_ip: "127.0.0.1".into(),
                locale: "zh".into(),
                level: "ERROR".into(),
                message: "任务 tcp-single 超过 30 秒未完成（超时，标记失败并继续下一项）".into(),
            },
        )
        .await;
        let content = std::fs::read_to_string(&summary_file).unwrap();
        assert!(
            content.contains("[ERROR] 任务 tcp-single 超过 30 秒未完成"),
            "前端超时错误应补写进运行日志"
        );

        server_tx.send(Some("stop".into())).unwrap();
        let _ = server_task.await;
    }
}
