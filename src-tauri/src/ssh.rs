//! 服务端页的 SSH 远程控制台。
//!
//! 在远端主机上开一个带 PTY 的交互式 shell，用于直接操作对端的 iperf3 server
//! （启动 / 中断 / 查看进程等），远端输出实时回传给前端控制台。
//! 使用纯 Rust 的 russh 实现，不依赖系统 ssh 客户端，与 riperf3 引擎一样零外部依赖。

use crate::runner::{current_locale, AppState};
use russh::keys::{
    check_known_hosts, load_secret_key, Error as KeysError, HashAlg, PrivateKeyWithHashAlg,
    PublicKey,
};
use russh::{client, ChannelMsg, Disconnect};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, RwLock,
    },
};
use tauri::{AppHandle, Emitter, State};
use tokio::{
    sync::{mpsc, Mutex as AsyncMutex},
    time::{timeout, Duration},
};
use uuid::Uuid;

/// 按界面语言选择消息文案（locale 为空时默认中文），与 runner 保持一致
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

/// 连接超时：DNS 解析 + TCP 握手 + SSH 协商
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
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
    fn append(&mut self, chunk: &str) -> u64 {
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
enum SshInput {
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
enum HostKey {
    /// 已登记在 known_hosts 中
    Known,
    /// 首次连接，known_hosts 里没有记录
    Unknown(String),
    /// known_hosts 中有记录但密钥已变化：可能是中间人攻击，直接拒绝
    Changed,
    /// known_hosts 不可读（文件不存在等），跳过校验
    Unreadable(String),
}

struct Handler {
    host: String,
    port: u16,
    result: Arc<Mutex<Option<HostKey>>>,
}

impl client::Handler for Handler {
    type Error = russh::Error;

    /// 主机密钥校验：与用户的 known_hosts 比对。
    /// 密钥变更时拒绝连接；首次连接（未登记）则放行并在控制台提示指纹，
    /// 供用户自行核对——本工具面向内网测试环境，不强制先行登记主机。
    async fn check_server_key(&mut self, key: &PublicKey) -> Result<bool, Self::Error> {
        let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
        let outcome = match check_known_hosts(&self.host, self.port, key) {
            Ok(true) => HostKey::Known,
            Ok(false) => HostKey::Unknown(fingerprint),
            Err(KeysError::KeyChanged { .. }) => HostKey::Changed,
            Err(error) => HostKey::Unreadable(format!("{error}")),
        };
        let accepted = !matches!(outcome, HostKey::Changed);
        if let Ok(mut slot) = self.result.lock() {
            *slot = Some(outcome);
        }
        Ok(accepted)
    }
}

// —— 远端输出解码：UTF-8 跨块拼接 + ANSI 转义序列剔除 ——

#[derive(Default, Clone, Copy, PartialEq)]
enum EscState {
    #[default]
    Text,
    /// 刚读到 ESC，等待后续类型字节
    Esc,
    /// CSI（ESC [ …）：终止于 0x40..=0x7E
    Csi,
    /// OSC / DCS / APC 等字符串型序列：终止于 BEL 或 ESC \
    Str,
    /// 字符串型序列中读到 ESC，等待 \
    StrEsc,
}

/// 把远端字节流转换成可直接显示的纯文本：
/// PTY 会输出颜色、光标定位等控制序列，这里逐字节剔除，只保留正文与 \r \n \b \t，
/// 由前端的行缓冲负责回车覆盖与退格。状态跨调用保留，因此序列被拆到两块也能正确处理。
#[derive(Default)]
struct Decoder {
    /// 上一块结尾不完整的 UTF-8 字节
    pending: Vec<u8>,
    state: EscState,
}

impl Decoder {
    fn feed(&mut self, chunk: &[u8]) -> String {
        self.pending.extend_from_slice(chunk);
        let mut decoded = String::new();
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(text) => {
                    decoded.push_str(text);
                    self.pending.clear();
                    break;
                }
                Err(error) => {
                    let valid = error.valid_up_to();
                    // valid_up_to() 之前的字节保证是合法 UTF-8
                    decoded.push_str(&String::from_utf8_lossy(&self.pending[..valid]));
                    match error.error_len() {
                        // 非法字节：以替换字符占位后继续解析剩余部分
                        Some(len) => {
                            decoded.push('\u{FFFD}');
                            self.pending.drain(..valid + len);
                        }
                        // 结尾是不完整的多字节序列：留到下一块再拼
                        None => {
                            self.pending.drain(..valid);
                            break;
                        }
                    }
                }
            }
        }
        self.strip(&decoded)
    }

    fn strip(&mut self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for ch in text.chars() {
            self.state = match self.state {
                EscState::Text => {
                    if ch == '\u{1b}' {
                        EscState::Esc
                    } else {
                        if ch == '\n' || ch == '\r' || ch == '\t' || ch == '\u{8}' || ch >= ' ' {
                            out.push(ch);
                        }
                        EscState::Text
                    }
                }
                EscState::Esc => match ch {
                    '[' => EscState::Csi,
                    ']' | 'P' | 'X' | '^' | '_' => EscState::Str,
                    // 其余为双字符转义（ESC = / ESC > 等），丢弃后回到正文
                    _ => EscState::Text,
                },
                EscState::Csi => {
                    if ('\u{40}'..='\u{7e}').contains(&ch) {
                        EscState::Text
                    } else {
                        EscState::Csi
                    }
                }
                EscState::Str => match ch {
                    '\u{7}' => EscState::Text,
                    '\u{1b}' => EscState::StrEsc,
                    _ => EscState::Str,
                },
                EscState::StrEsc => {
                    if ch == '\\' {
                        EscState::Text
                    } else {
                        EscState::Str
                    }
                }
            };
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::{
        run_session, Decoder, Sink, SshEvent, SshInput, SshRequest, SshScrollback, SCROLLBACK_LIMIT,
    };
    use russh::keys::ssh_key::rand_core::{Infallible, TryCryptoRng, TryRng};
    use russh::server::{self, Auth, Msg, Server as _, Session};
    use russh::{Channel, ChannelId};
    use std::sync::{Arc, Mutex, RwLock};
    use tokio::sync::mpsc;
    use tokio::time::{sleep, timeout, Duration};

    #[test]
    fn keeps_plain_text_and_line_control_chars() {
        let mut decoder = Decoder::default();
        assert_eq!(decoder.feed(b"iperf3\r\n- - - -\n"), "iperf3\r\n- - - -\n");
        // 响铃等其余控制字符不应进入控制台
        assert_eq!(decoder.feed(b"a\x07b"), "ab");
    }

    #[test]
    fn strips_ansi_sequences() {
        let mut decoder = Decoder::default();
        assert_eq!(decoder.feed(b"\x1b[32mgreen\x1b[0m done"), "green done");
        // OSC（设置窗口标题）以 BEL 结束
        assert_eq!(decoder.feed(b"\x1b]0;user@host\x07$ "), "$ ");
        // OSC 也可以用 ESC \ 结束
        assert_eq!(decoder.feed(b"\x1b]0;t\x1b\\ok"), "ok");
    }

    #[test]
    fn resumes_sequences_split_across_chunks() {
        let mut decoder = Decoder::default();
        assert_eq!(decoder.feed(b"\x1b[3"), "");
        assert_eq!(decoder.feed(b"2mok"), "ok");
    }

    #[test]
    fn resumes_utf8_split_across_chunks() {
        let mut decoder = Decoder::default();
        // “中” = E4 B8 AD，被拆成两块
        assert_eq!(decoder.feed(&[0xe4, 0xb8]), "");
        assert_eq!(decoder.feed(&[0xad]), "中");
    }

    #[test]
    fn replaces_invalid_utf8_and_continues() {
        let mut decoder = Decoder::default();
        assert_eq!(decoder.feed(&[b'a', 0xff, b'b']), "a\u{fffd}b");
    }

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
        // 截断后不留半行，且累计偏移仍是全部输出的字节数
        assert!(buffer.text.starts_with('x'));
        assert!(buffer.end_offset > SCROLLBACK_LIMIT as u64);
    }

    // —— 端到端：内嵌一个最小 SSH 服务端，跑通「连接 → 认证 → 申请 PTY → 开 shell →
    //    收发数据 → 断开」的完整链路（本地回环，无需外部 sshd）——

    const TEST_USER: &str = "tester";
    const TEST_PASSWORD: &str = "s3cret";

    /// 测试专用的确定性 RNG：只用于给内嵌测试服务端生成一次性的回环主机密钥，
    /// 不参与任何真实连接的密钥材料生成
    struct TestRng(u64);
    impl TryRng for TestRng {
        type Error = Infallible;
        fn try_next_u32(&mut self) -> Result<u32, Infallible> {
            Ok(self.try_next_u64()? as u32)
        }
        fn try_next_u64(&mut self) -> Result<u64, Infallible> {
            // xorshift64*：足够生成测试用密钥
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            Ok(self.0.wrapping_mul(0x2545_f491_4f6c_dd1d))
        }
        fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Infallible> {
            for chunk in dst.chunks_mut(8) {
                let bytes = self.try_next_u64()?.to_le_bytes();
                chunk.copy_from_slice(&bytes[..chunk.len()]);
            }
            Ok(())
        }
    }
    impl TryCryptoRng for TestRng {}

    /// 最小 SSH 服务端：只接受约定的用户名 / 密码，开 shell 后回一行带 ANSI 颜色的提示符，
    /// 收到命令则回一行响应，收到 Ctrl+C（0x03）则回 "^C"
    #[derive(Clone)]
    struct TestServer;

    impl server::Server for TestServer {
        type Handler = Self;
        fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
            self.clone()
        }
    }

    impl server::Handler for TestServer {
        type Error = russh::Error;

        async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
            if user == TEST_USER && password == TEST_PASSWORD {
                Ok(Auth::Accept)
            } else {
                Ok(Auth::reject())
            }
        }

        async fn channel_open_session(
            &mut self,
            _channel: Channel<Msg>,
            reply: server::ChannelOpenHandle,
            _session: &mut Session,
        ) -> Result<(), Self::Error> {
            reply.accept().await;
            Ok(())
        }

        async fn pty_request(
            &mut self,
            channel: ChannelId,
            _term: &str,
            _cols: u32,
            _rows: u32,
            _pix_width: u32,
            _pix_height: u32,
            _modes: &[(russh::Pty, u32)],
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            session.channel_success(channel)?;
            Ok(())
        }

        async fn shell_request(
            &mut self,
            channel: ChannelId,
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            session.channel_success(channel)?;
            // 带 ANSI 颜色的提示符：客户端应把转义序列剔除后只留正文
            session.data(channel, b"\x1b[32mtester@host\x1b[0m:~$ ".to_vec())?;
            Ok(())
        }

        async fn data(
            &mut self,
            channel: ChannelId,
            data: &[u8],
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            let reply = if data == [3] {
                "^C\r\n".to_string()
            } else {
                let command = String::from_utf8_lossy(data);
                format!("{}iperf3 3.16\r\n", command.trim_end_matches('\n'))
            };
            session.data(channel, reply.into_bytes())?;
            Ok(())
        }
    }

    /// 启动内嵌测试服务端，返回其监听端口
    async fn start_test_server() -> u16 {
        let key =
            russh::keys::PrivateKey::random(&mut TestRng(0x5eed), russh::keys::Algorithm::Ed25519)
                .unwrap();
        let config = Arc::new(server::Config {
            auth_rejection_time: Duration::from_millis(50),
            keys: vec![key],
            ..Default::default()
        });
        let socket = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = socket.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut server = TestServer;
            let _ = server.run_on_socket(config, &socket).await;
        });
        port
    }

    fn request(port: u16, password: &str) -> SshRequest {
        SshRequest {
            host: "127.0.0.1".into(),
            port,
            username: TEST_USER.into(),
            auth_method: "password".into(),
            password: password.into(),
            private_key_path: String::new(),
            passphrase: String::new(),
            cols: 100,
            rows: 30,
        }
    }

    /// 事件收集器 + 会话运行环境
    struct Harness {
        events: Arc<Mutex<Vec<SshEvent>>>,
        scrollback: Arc<Mutex<SshScrollback>>,
        input: mpsc::UnboundedSender<SshInput>,
        task: tokio::task::JoinHandle<()>,
    }

    fn spawn_session(request: SshRequest) -> Harness {
        let events = Arc::new(Mutex::new(Vec::new()));
        let collected = events.clone();
        let sink: Sink = Arc::new(move |event| collected.lock().unwrap().push(event));
        let scrollback = Arc::new(Mutex::new(SshScrollback::default()));
        let (input, rx) = mpsc::unbounded_channel();
        let locale = Arc::new(RwLock::new("en".to_string()));
        let task = tokio::spawn(run_session(
            sink,
            "test-session".into(),
            request,
            rx,
            scrollback.clone(),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            locale,
        ));
        Harness {
            events,
            scrollback,
            input,
            task,
        }
    }

    impl Harness {
        fn types(&self) -> Vec<String> {
            self.events
                .lock()
                .unwrap()
                .iter()
                .map(|e| e.event_type.clone())
                .collect()
        }
        fn console(&self) -> String {
            self.scrollback.lock().unwrap().text.clone()
        }
        /// 轮询等待控制台输出包含指定内容
        async fn wait_for(&self, needle: &str) {
            let deadline = timeout(Duration::from_secs(15), async {
                while !self.console().contains(needle) {
                    sleep(Duration::from_millis(20)).await;
                }
            });
            assert!(
                deadline.await.is_ok(),
                "未等到控制台输出 {needle:?}，当前内容：{:?}",
                self.console()
            );
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn opens_a_remote_shell_and_streams_output() {
        let port = start_test_server().await;
        let harness = spawn_session(request(port, TEST_PASSWORD));

        // shell 就绪：提示符到达，且 ANSI 颜色序列已被剔除
        harness.wait_for("tester@host:~$ ").await;
        assert!(!harness.console().contains('\u{1b}'));
        assert!(harness.types().iter().any(|t| t == "status"));

        // 在控制台里执行一条命令，远端响应实时回传
        harness
            .input
            .send(SshInput::Data(b"iperf3 --version\n".to_vec()))
            .unwrap();
        harness.wait_for("iperf3 3.16").await;
        assert!(harness.console().contains("iperf3 --version"));

        // Ctrl+C 作为原始字节送达远端
        harness.input.send(SshInput::Data(vec![3])).unwrap();
        harness.wait_for("^C").await;

        // 数据块的偏移量连续递增，前端据此与回放缓冲去重
        let offsets: Vec<u64> = harness
            .events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| e.offset)
            .collect();
        assert!(
            offsets.windows(2).all(|w| w[0] < w[1]),
            "offsets: {offsets:?}"
        );

        // 主动断开：会话正常收尾并发出 closed 事件
        harness.input.send(SshInput::Close).unwrap();
        let Harness { task, events, .. } = harness;
        timeout(Duration::from_secs(10), task)
            .await
            .unwrap()
            .unwrap();
        let types: Vec<String> = events
            .lock()
            .unwrap()
            .iter()
            .map(|e| e.event_type.clone())
            .collect();
        assert_eq!(types.last().map(String::as_str), Some("closed"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn reports_authentication_failure() {
        let port = start_test_server().await;
        let Harness { task, events, .. } = spawn_session(request(port, "wrong-password"));
        timeout(Duration::from_secs(15), task)
            .await
            .unwrap()
            .unwrap();

        let events = events.lock().unwrap();
        let last = events.last().expect("应至少产生一个事件");
        assert_eq!(last.event_type, "error");
        assert!(
            last.message
                .as_deref()
                .unwrap_or_default()
                .contains("authentication failed"),
            "unexpected message: {:?}",
            last.message
        );
    }
}

/// 事件汇：桌面端把事件广播给所有窗口，测试中换成收集器即可脱离 Tauri 运行时
type Sink = Arc<dyn Fn(SshEvent) + Send + Sync>;

fn tauri_sink(app: AppHandle) -> Sink {
    Arc::new(move |event| {
        let _ = app.emit("ssh-event", event);
    })
}

fn emit(
    sink: &Sink,
    session_id: &str,
    event_type: &str,
    level: Option<&str>,
    message: Option<String>,
    offset: Option<u64>,
) {
    sink(SshEvent {
        session_id: session_id.into(),
        event_type: event_type.into(),
        level: level.map(Into::into),
        message,
        offset,
    });
}

/// 连接阶段（connecting / connected）：前端据此切换按钮与状态灯
fn emit_status(sink: &Sink, session_id: &str, status: &str) {
    emit(sink, session_id, "status", None, Some(status.into()), None);
}

/// 生命周期提示：同时写入应用日志面板
fn emit_log(sink: &Sink, session_id: &str, level: &str, message: String) {
    emit(sink, session_id, "log", Some(level), Some(message), None);
}

/// 会话结束（closed / error）
fn emit_end(sink: &Sink, session_id: &str, event_type: &str, message: String) {
    let level = if event_type == "error" {
        "ERROR"
    } else {
        "INFO"
    };
    emit(
        sink,
        session_id,
        event_type,
        Some(level),
        Some(message),
        None,
    );
}

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

async fn run_session(
    sink: Sink,
    session_id: String,
    request: SshRequest,
    mut rx: mpsc::UnboundedReceiver<SshInput>,
    scrollback: Arc<Mutex<SshScrollback>>,
    connected: Arc<AtomicBool>,
    locale_handle: Arc<RwLock<String>>,
) {
    let locale = current_locale(&locale_handle);
    emit_status(&sink, &session_id, "connecting");
    emit_log(
        &sink,
        &session_id,
        "INFO",
        tr_format!(
            locale,
            "正在连接 SSH {}@{}:{}…",
            "Connecting to SSH {}@{}:{}…",
            request.username,
            request.host,
            request.port
        ),
    );

    let config = Arc::new(client::Config {
        // 30 秒心跳、连续 3 次无应答即判定断线，避免远端拔网线后界面一直显示已连接
        keepalive_interval: Some(Duration::from_secs(30)),
        keepalive_max: 3,
        ..Default::default()
    });
    let host_key = Arc::new(Mutex::new(None));
    let handler = Handler {
        host: request.host.trim().to_string(),
        port: request.port,
        result: host_key.clone(),
    };
    let connecting = client::connect(
        config,
        (request.host.trim().to_string(), request.port),
        handler,
    );
    let mut session = match timeout(CONNECT_TIMEOUT, connecting).await {
        Err(_) => {
            emit_end(
                &sink,
                &session_id,
                "error",
                tr_format!(
                    locale,
                    "连接 {}:{} 超时（{} 秒）",
                    "Connection to {}:{} timed out after {}s",
                    request.host,
                    request.port,
                    CONNECT_TIMEOUT.as_secs()
                ),
            );
            return;
        }
        Ok(Err(error)) => {
            // 主机密钥变更会让握手直接失败，此时给出更明确的原因
            let changed = matches!(host_key.lock().as_deref(), Ok(Some(HostKey::Changed)));
            let message = if changed {
                tr_format!(
                    locale,
                    "主机密钥与 known_hosts 中的记录不一致，已拒绝连接（可能是中间人攻击，或远端重装过系统）",
                    "The host key does not match the record in known_hosts; the connection was refused (possible man-in-the-middle attack, or the host was reinstalled)"
                )
            } else {
                tr_format!(
                    locale,
                    "SSH 连接失败：{}",
                    "SSH connection failed: {}",
                    error
                )
            };
            emit_end(&sink, &session_id, "error", message);
            return;
        }
        Ok(Ok(session)) => session,
    };

    // 主机密钥校验提示（首次连接时显示指纹供用户核对）
    if let Ok(slot) = host_key.lock() {
        match slot.as_ref() {
            Some(HostKey::Known) => emit_log(
                &sink,
                &session_id,
                "INFO",
                tr_format!(
                    locale,
                    "主机密钥已在 known_hosts 中登记",
                    "Host key matches the known_hosts record"
                ),
            ),
            Some(HostKey::Unknown(fingerprint)) => emit_log(
                &sink,
                &session_id,
                "WARN",
                tr_format!(
                    locale,
                    "首次连接该主机，known_hosts 中没有记录，请核对指纹：{}",
                    "First connection to this host — no known_hosts record. Verify the fingerprint: {}",
                    fingerprint
                ),
            ),
            Some(HostKey::Unreadable(error)) => emit_log(
                &sink,
                &session_id,
                "WARN",
                tr_format!(
                    locale,
                    "无法读取 known_hosts（{}），已跳过主机密钥校验",
                    "known_hosts is unreadable ({}); host key verification was skipped",
                    error
                ),
            ),
            _ => {}
        }
    }

    // —— 认证 ——
    let authenticated = if request.auth_method == "key" {
        let passphrase = (!request.passphrase.is_empty()).then_some(request.passphrase.as_str());
        match load_secret_key(request.private_key_path.trim(), passphrase) {
            Ok(key) => {
                let hash = session
                    .best_supported_rsa_hash()
                    .await
                    .ok()
                    .flatten()
                    .flatten();
                session
                    .authenticate_publickey(
                        request.username.trim(),
                        PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                    )
                    .await
            }
            Err(error) => {
                emit_end(
                    &sink,
                    &session_id,
                    "error",
                    tr_format!(
                        locale,
                        "私钥读取失败：{}（若私钥带口令，请填写口令）",
                        "Failed to load the private key: {} (fill in the passphrase if the key is encrypted)",
                        error
                    ),
                );
                return;
            }
        }
    } else {
        session
            .authenticate_password(request.username.trim(), request.password.as_str())
            .await
    };
    match authenticated {
        Ok(result) if result.success() => {}
        Ok(_) => {
            emit_end(
                &sink,
                &session_id,
                "error",
                tr_format!(
                    locale,
                    "SSH 认证失败，请检查用户名与{}",
                    "SSH authentication failed — check the username and {}",
                    tr(
                        &locale,
                        if request.auth_method == "key" {
                            "私钥"
                        } else {
                            "密码"
                        },
                        if request.auth_method == "key" {
                            "private key"
                        } else {
                            "password"
                        }
                    )
                ),
            );
            return;
        }
        Err(error) => {
            emit_end(
                &sink,
                &session_id,
                "error",
                tr_format!(
                    locale,
                    "SSH 认证出错：{}",
                    "SSH authentication error: {}",
                    error
                ),
            );
            return;
        }
    }

    // —— 打开交互式 shell ——
    let mut channel = match session.channel_open_session().await {
        Ok(channel) => channel,
        Err(error) => {
            emit_end(
                &sink,
                &session_id,
                "error",
                tr_format!(
                    locale,
                    "打开 SSH 通道失败：{}",
                    "Failed to open the SSH channel: {}",
                    error
                ),
            );
            return;
        }
    };
    let cols = request.cols.clamp(40, 500);
    let rows = request.rows.clamp(10, 200);
    if let Err(error) = channel
        .request_pty(false, "xterm", cols, rows, 0, 0, &[])
        .await
    {
        emit_end(
            &sink,
            &session_id,
            "error",
            tr_format!(
                locale,
                "申请远端终端失败：{}",
                "Failed to request a remote terminal: {}",
                error
            ),
        );
        return;
    }
    if let Err(error) = channel.request_shell(true).await {
        emit_end(
            &sink,
            &session_id,
            "error",
            tr_format!(
                locale,
                "启动远端 shell 失败：{}",
                "Failed to start the remote shell: {}",
                error
            ),
        );
        return;
    }

    // 先置位再广播：前端若在事件之前就拿到会话 id 并拉取快照，也能读到已连接状态
    connected.store(true, Ordering::SeqCst);
    emit_status(&sink, &session_id, "connected");
    emit_log(
        &sink,
        &session_id,
        "INFO",
        tr_format!(
            locale,
            "SSH 已连接：{}@{}:{}",
            "SSH connected: {}@{}:{}",
            request.username,
            request.host,
            request.port
        ),
    );

    // —— 转发循环：前端输入 → 远端 shell，远端输出 → 前端控制台 ——
    let mut decoder = Decoder::default();
    // 循环以「结束原因」作为返回值，每条退出路径各自给出文案（按结束时的界面语言）
    let reason = loop {
        tokio::select! {
            input = rx.recv() => match input {
                None | Some(SshInput::Close) => {
                    let _ = channel.eof().await;
                    break tr(&current_locale(&locale_handle), "已断开连接", "Disconnected").to_string();
                }
                Some(SshInput::Data(bytes)) => {
                    if channel.data_bytes(bytes).await.is_err() {
                        break tr(&current_locale(&locale_handle), "连接已中断", "The connection was interrupted").to_string();
                    }
                }
                Some(SshInput::Resize { cols, rows }) => {
                    let _ = channel.window_change(cols.clamp(40, 500), rows.clamp(10, 200), 0, 0).await;
                }
            },
            message = channel.wait() => match message {
                Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                    let text = decoder.feed(&data);
                    if !text.is_empty() {
                        let offset = scrollback.lock().map(|mut buffer| buffer.append(&text)).unwrap_or(0);
                        emit(&sink, &session_id, "data", None, Some(text), Some(offset));
                    }
                }
                Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => {
                    break tr(&current_locale(&locale_handle), "远端已关闭会话", "The remote host closed the session").to_string();
                }
                Some(_) => {}
            },
        }
    };

    connected.store(false, Ordering::SeqCst);
    let _ = channel.close().await;
    let _ = session
        .disconnect(Disconnect::ByApplication, "", "en-US")
        .await;
    emit_end(&sink, &session_id, "closed", reason);
}
