//! 服务端页的 SSH 远程控制台。
//!
//! 在远端主机上开一个带 PTY 的交互式 shell，用于直接操作对端的 iperf3 server
//! （启动 / 中断 / 查看进程等），远端输出实时回传给前端控制台。
//! 使用纯 Rust 的 russh 实现，不依赖系统 ssh 客户端，与 riperf3 引擎一样零外部依赖。

pub(crate) use crate::runner::tr;
use crate::runner::{current_locale, AppState};
use crate::ssh_session::{run_session, tauri_sink};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tauri::{AppHandle, State};
use tokio::sync::{mpsc, Mutex as AsyncMutex};
use uuid::Uuid;

/// 连接超时：DNS 解析 + TCP 握手 + SSH 协商
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// 回放缓冲上限：新打开的窗口据此恢复控制台内容（超出部分从最旧的整行开始丢弃）
const SCROLLBACK_LIMIT: usize = 128 * 1024;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshRequest {
    pub host: String,
    pub port: u16,
    pub username: String,
    /// 认证方式：password（密码）/ key（私钥）
    #[serde(default)]
    pub auth_method: String,
    /// 登录密码：仅在内存中流转，前端不写入本地存储、不随配置导出
    #[serde(default)]
    pub password: String,
    /// 私钥文件路径（auth_method = key 时使用）
    #[serde(default)]
    pub private_key_path: String,
    /// 私钥口令（同样不落盘）
    #[serde(default)]
    pub passphrase: String,
    /// 远端 PTY 尺寸（前端按控制台可视区域估算）
    #[serde(default)]
    pub cols: u32,
    #[serde(default)]
    pub rows: u32,
}

/// 控制台事件（前端监听 `ssh-event`）：
/// - `status`  连接阶段（connecting / connected）
/// - `log`     生命周期提示（写入应用日志面板）
/// - `data`    远端输出文本块，带流内偏移量
/// - `closed`  会话正常结束
/// - `error`   会话异常结束
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshEvent {
    pub session_id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// data 事件专用：本块文本在会话输出流中的起始偏移，
    /// 新窗口拉取回放缓冲后据此丢弃已包含在回放里的实时块
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
}

/// 回放缓冲：新打开 / 拖出的窗口靠它恢复控制台已有内容
#[derive(Debug, Default, Clone)]
pub struct SshScrollback {
    pub text: String,
    /// 会话开始至今累计输出的字节数（= 下一个 data 块的 offset）
    pub end_offset: u64,
}

/// 会话快照：回放缓冲 + 当前连接状态。
/// 状态一并返回是必要的——同机连接时 shell 可能在 `ssh_connect` 返回会话 id 之前就已就绪，
/// 那一刻发出的 connected 事件会因前端还不知道会话 id 而被丢弃
#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshSnapshot {
    pub text: String,
    pub end_offset: u64,
    /// shell 是否已就绪（可交互）
    pub connected: bool,
}

impl SshScrollback {
    /// 追加一块输出，返回该块的起始偏移
    pub(crate) fn append(&mut self, chunk: &str) -> u64 {
        let offset = self.end_offset;
        self.end_offset += chunk.len() as u64;
        self.text.push_str(chunk);
        if self.text.len() > SCROLLBACK_LIMIT {
            let mut cut = self.text.len() - SCROLLBACK_LIMIT;
            while cut < self.text.len() && !self.text.is_char_boundary(cut) {
                cut += 1;
            }
            // 从换行处截断，避免顶部留下半行
            let start = self.text[cut..]
                .find('\n')
                .map(|i| cut + i + 1)
                .unwrap_or(cut);
            self.text = self.text[start..].to_string();
        }
        offset
    }
}

/// 发往远端 shell 的指令
pub(crate) enum SshInput {
    Data(Vec<u8>),
    Resize { cols: u32, rows: u32 },
    Close,
}

struct SshSession {
    input: mpsc::UnboundedSender<SshInput>,
    scrollback: Arc<Mutex<SshScrollback>>,
    /// shell 就绪标志（连上远端并拿到交互式 shell 后置位）
    connected: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct SshState {
    sessions: Arc<AsyncMutex<HashMap<String, SshSession>>>,
}

/// 主机密钥校验结果（在 check_server_key 回调里记录，连接建立后翻译成提示文案）
pub(crate) enum HostKey {
    /// 已登记在 known_hosts 中
    Known,
    /// 首次连接，known_hosts 里没有记录
    Unknown(String),
    /// known_hosts 中有记录但密钥已变化：可能是中间人攻击，直接拒绝
    Changed,
    /// known_hosts 不可读（文件不存在等），跳过校验
    Unreadable(String),
}

#[cfg(test)]
mod tests {
    use super::{SshScrollback, SCROLLBACK_LIMIT};

    #[test]
    fn scrollback_reports_offsets_and_trims_at_line_boundary() {
        let mut buffer = SshScrollback::default();
        assert_eq!(buffer.append("first\n"), 0);
        assert_eq!(buffer.append("second\n"), 6);
        assert_eq!(buffer.end_offset, 13);

        let line = "x".repeat(99) + "\n";
        for _ in 0..(SCROLLBACK_LIMIT / 100 + 10) {
            buffer.append(&line);
        }
        assert!(buffer.text.len() <= SCROLLBACK_LIMIT);
        assert!(buffer.text.starts_with('x'));
        assert!(buffer.end_offset > SCROLLBACK_LIMIT as u64);
    }
}

/// 事件汇：桌面端把事件广播给所有窗口，测试中换成收集器即可脱离 Tauri 运行时
pub(crate) type Sink = Arc<dyn Fn(SshEvent) + Send + Sync>;

#[tauri::command]
pub async fn ssh_connect(
    app: AppHandle,
    state: State<'_, SshState>,
    app_state: State<'_, AppState>,
    request: SshRequest,
) -> Result<String, String> {
    let locale = current_locale(&app_state.locale);
    if request.host.trim().is_empty() {
        return Err(tr(&locale, "请输入 SSH 主机地址", "Enter the SSH host").into());
    }
    if request.port == 0 {
        return Err(tr(
            &locale,
            "SSH 端口应在 1–65535 之间",
            "SSH port must be between 1 and 65535",
        )
        .into());
    }
    if request.username.trim().is_empty() {
        return Err(tr(&locale, "请输入 SSH 用户名", "Enter the SSH username").into());
    }
    if request.auth_method == "key" {
        if request.private_key_path.trim().is_empty() {
            return Err(tr(
                &locale,
                "请选择用于登录的私钥文件",
                "Select the private key file to log in with",
            )
            .into());
        }
    } else if request.password.is_empty() {
        return Err(tr(&locale, "请输入 SSH 登录密码", "Enter the SSH password").into());
    }

    let session_id = Uuid::new_v4().to_string();
    let scrollback = Arc::new(Mutex::new(SshScrollback::default()));
    let connected = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::unbounded_channel();
    state.sessions.lock().await.insert(
        session_id.clone(),
        SshSession {
            input: tx,
            scrollback: scrollback.clone(),
            connected: connected.clone(),
        },
    );
    let sessions = state.sessions.clone();
    let locale_handle = app_state.locale.clone();
    let spawned_id = session_id.clone();
    let sink = tauri_sink(app);
    tauri::async_runtime::spawn(async move {
        run_session(
            sink,
            spawned_id.clone(),
            request,
            rx,
            scrollback,
            connected,
            locale_handle,
        )
        .await;
        sessions.lock().await.remove(&spawned_id);
    });
    Ok(session_id)
}

/// 向远端 shell 写入数据（命令文本、Ctrl+C 等控制字符）
#[tauri::command]
pub async fn ssh_send(
    state: State<'_, SshState>,
    session_id: String,
    data: String,
) -> Result<(), String> {
    send(&state, &session_id, SshInput::Data(data.into_bytes())).await
}

/// 同步远端 PTY 尺寸，让 top/iperf3 等按控制台宽度换行
#[tauri::command]
pub async fn ssh_resize(
    state: State<'_, SshState>,
    session_id: String,
    cols: u32,
    rows: u32,
) -> Result<(), String> {
    send(&state, &session_id, SshInput::Resize { cols, rows }).await
}

#[tauri::command]
pub async fn ssh_disconnect(state: State<'_, SshState>, session_id: String) -> Result<(), String> {
    send(&state, &session_id, SshInput::Close).await
}

/// 读取会话快照（回放缓冲 + 连接状态）：新开的窗口、以及刚拿到会话 id 的发起方
/// 都据此把控制台内容与连接状态补齐
#[tauri::command]
pub async fn ssh_scrollback(
    state: State<'_, SshState>,
    session_id: String,
) -> Result<SshSnapshot, String> {
    let map = state.sessions.lock().await;
    let session = map
        .get(&session_id)
        .ok_or_else(|| "SSH 会话不存在或已结束".to_string())?;
    let buffer = session
        .scrollback
        .lock()
        .map_err(|_| "读取控制台缓冲失败".to_string())?;
    Ok(SshSnapshot {
        text: buffer.text.clone(),
        end_offset: buffer.end_offset,
        connected: session.connected.load(Ordering::SeqCst),
    })
}

async fn send(
    state: &State<'_, SshState>,
    session_id: &str,
    input: SshInput,
) -> Result<(), String> {
    let map = state.sessions.lock().await;
    let session = map
        .get(session_id)
        .ok_or_else(|| "SSH 会话不存在或已结束".to_string())?;
    session
        .input
        .send(input)
        .map_err(|_| "SSH 会话已断开".to_string())
}
