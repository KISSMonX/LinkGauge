//! SSH 会话运行器：从 ssh.rs 拆分出的会话生命周期管理。
//!
//! 包含 run_session 主循环、Handler（主机密钥校验）、事件汇（Sink）
//! 以及端到端集成测试。

use crate::i18n::current_locale;
use crate::ssh::{tr, Sink, SshEvent, SshInput, SshRequest, SshScrollback, CONNECT_TIMEOUT};
use crate::ssh_decoder::Decoder;
use russh::keys::{
    check_known_hosts, load_secret_key, Error as KeysError, HashAlg, PrivateKeyWithHashAlg,
    PublicKey,
};
use russh::{client, ChannelMsg, Disconnect};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, RwLock,
};
use tauri::Emitter;
use tokio::sync::mpsc;
use tokio::time::{timeout, Duration};

// ---------------------------------------------------------------------------
// 事件汇：生产环境广播给 Tauri 所有窗口，测试环境收集到 Vec
// ---------------------------------------------------------------------------

pub(crate) fn tauri_sink(app: tauri::AppHandle) -> Sink {
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

pub(crate) fn emit_status(sink: &Sink, session_id: &str, status: &str) {
    emit(sink, session_id, "status", None, Some(status.into()), None);
}

pub(crate) fn emit_log(sink: &Sink, session_id: &str, level: &str, message: String) {
    emit(sink, session_id, "log", Some(level), Some(message), None);
}

pub(crate) fn emit_end(sink: &Sink, session_id: &str, event_type: &str, message: String) {
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

// ---------------------------------------------------------------------------
// 主机密钥校验
// ---------------------------------------------------------------------------

pub(crate) struct Handler {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) result: Arc<Mutex<Option<crate::ssh::HostKey>>>,
}

impl client::Handler for Handler {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKey) -> Result<bool, Self::Error> {
        let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
        let outcome = match check_known_hosts(&self.host, self.port, key) {
            Ok(true) => crate::ssh::HostKey::Known,
            Ok(false) => crate::ssh::HostKey::Unknown(fingerprint),
            Err(KeysError::KeyChanged { .. }) => crate::ssh::HostKey::Changed,
            Err(error) => crate::ssh::HostKey::Unreadable(format!("{error}")),
        };
        let accepted = !matches!(outcome, crate::ssh::HostKey::Changed);
        if let Ok(mut slot) = self.result.lock() {
            *slot = Some(outcome);
        }
        Ok(accepted)
    }
}

// ---------------------------------------------------------------------------
// 会话主循环
// ---------------------------------------------------------------------------

/// 执行 SSH 认证（密码或私钥）。成功返回 `true`，失败时通过 sink 发送错误事件并返回 `false`。
async fn authenticate_session(
    sink: &Sink,
    session_id: &str,
    request: &SshRequest,
    locale: &str,
    session: &mut russh::client::Handle<Handler>,
) -> bool {
    let authenticated = if request.auth_method == "key" {
        let pp = (!request.passphrase.is_empty()).then_some(request.passphrase.as_str());
        match load_secret_key(request.private_key_path.trim(), pp) {
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
                    sink,
                    session_id,
                    "error",
                    crate::tr_format!(
                        locale,
                        "私钥读取失败：{}",
                        "Failed to load the private key: {}",
                        error
                    ),
                );
                return false;
            }
        }
    } else {
        session
            .authenticate_password(request.username.trim(), request.password.as_str())
            .await
    };
    match authenticated {
        Ok(r) if r.success() => true,
        Ok(_) => {
            let method = if request.auth_method == "key" {
                "私钥"
            } else {
                "密码"
            };
            let en_method = if request.auth_method == "key" {
                "private key"
            } else {
                "password"
            };
            emit_end(
                sink,
                session_id,
                "error",
                crate::tr_format!(
                    locale,
                    "SSH 认证失败，请检查用户名与{}",
                    "SSH authentication failed — check username and {}",
                    tr(locale, method, en_method)
                ),
            );
            false
        }
        Err(error) => {
            emit_end(
                sink,
                session_id,
                "error",
                crate::tr_format!(
                    locale,
                    "SSH 认证出错：{}",
                    "SSH authentication error: {}",
                    error
                ),
            );
            false
        }
    }
}

pub(crate) async fn run_session(
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
        crate::tr_format!(
            locale,
            "正在连接 SSH {}@{}:{}…",
            "Connecting to SSH {}@{}:{}…",
            request.username,
            request.host,
            request.port
        ),
    );

    let host = request.host.trim().to_string();
    let config = Arc::new(client::Config {
        keepalive_interval: Some(Duration::from_secs(30)),
        keepalive_max: 3,
        ..Default::default()
    });
    let host_key = Arc::new(Mutex::new(None));
    let handler = Handler {
        host: host.clone(),
        port: request.port,
        result: host_key.clone(),
    };
    let connecting = client::connect(config, (host, request.port), handler);
    let mut session = match timeout(CONNECT_TIMEOUT, connecting).await {
        Err(_) => {
            emit_end(
                &sink,
                &session_id,
                "error",
                crate::tr_format!(
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
            let changed = matches!(
                host_key.lock().as_deref(),
                Ok(Some(crate::ssh::HostKey::Changed))
            );
            let message = if changed {
                crate::tr_format!(
                    locale,
                    "主机密钥与 known_hosts 中的记录不一致，已拒绝连接",
                    "The host key does not match the record in known_hosts; the connection was refused"
                )
            } else {
                crate::tr_format!(
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

    // 主机密钥校验提示
    if let Ok(slot) = host_key.lock() {
        match slot.as_ref() {
            Some(crate::ssh::HostKey::Known) => emit_log(
                &sink,
                &session_id,
                "INFO",
                crate::tr_format!(
                    locale,
                    "主机密钥已在 known_hosts 中登记",
                    "Host key matches the known_hosts record"
                ),
            ),
            Some(crate::ssh::HostKey::Unknown(fp)) => emit_log(
                &sink,
                &session_id,
                "WARN",
                crate::tr_format!(
                    locale,
                    "首次连接该主机，known_hosts 中没有记录，请核对指纹：{}",
                    "First connection — no known_hosts record. Verify fingerprint: {}",
                    fp
                ),
            ),
            Some(crate::ssh::HostKey::Unreadable(error)) => emit_log(
                &sink,
                &session_id,
                "WARN",
                crate::tr_format!(
                    locale,
                    "无法读取 known_hosts（{}），已跳过主机密钥校验",
                    "known_hosts is unreadable ({}); host key verification skipped",
                    error
                ),
            ),
            _ => {}
        }
    }

    // 认证
    if !authenticate_session(&sink, &session_id, &request, &locale, &mut session).await {
        return;
    }

    // 打开 shell
    let mut channel = match session.channel_open_session().await {
        Ok(ch) => ch,
        Err(error) => {
            emit_end(
                &sink,
                &session_id,
                "error",
                crate::tr_format!(
                    locale,
                    "打开 SSH 通道失败：{}",
                    "Failed to open SSH channel: {}",
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
            crate::tr_format!(
                locale,
                "申请远端终端失败：{}",
                "Failed to request remote terminal: {}",
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
            crate::tr_format!(
                locale,
                "启动远端 shell 失败：{}",
                "Failed to start remote shell: {}",
                error
            ),
        );
        return;
    }

    connected.store(true, Ordering::SeqCst);
    emit_status(&sink, &session_id, "connected");
    emit_log(
        &sink,
        &session_id,
        "INFO",
        crate::tr_format!(
            locale,
            "SSH 已连接：{}@{}:{}",
            "SSH connected: {}@{}:{}",
            request.username,
            request.host,
            request.port
        ),
    );

    // 转发循环
    let mut decoder = Decoder::default();
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
                        let offset = scrollback.lock().map(|mut b| b.append(&text)).unwrap_or(0);
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

// ---------------------------------------------------------------------------
// 端到端测试：内嵌最小 SSH 服务端
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::run_session;
    use crate::ssh::{Sink, SshEvent, SshInput, SshRequest, SshScrollback};
    use russh::keys::ssh_key::rand_core::{Infallible, TryCryptoRng, TryRng};
    use russh::server::{self, Auth, Msg, Server as _, Session};
    use russh::{Channel, ChannelId};
    use std::sync::{Arc, Mutex, RwLock};
    use tokio::sync::mpsc;
    use tokio::time::{sleep, timeout, Duration};

    const TEST_USER: &str = "tester";
    const TEST_PASSWORD: &str = "s3cret";

    struct TestRng(u64);
    impl TryRng for TestRng {
        type Error = Infallible;
        fn try_next_u32(&mut self) -> Result<u32, Infallible> {
            Ok(self.try_next_u64()? as u32)
        }
        fn try_next_u64(&mut self) -> Result<u64, Infallible> {
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
            _: Channel<Msg>,
            reply: server::ChannelOpenHandle,
            _: &mut Session,
        ) -> Result<(), Self::Error> {
            reply.accept().await;
            Ok(())
        }
        async fn pty_request(
            &mut self,
            channel: ChannelId,
            _: &str,
            _: u32,
            _: u32,
            _: u32,
            _: u32,
            _: &[(russh::Pty, u32)],
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
                "^C\r\n".into()
            } else {
                format!(
                    "{}iperf3 3.16\r\n",
                    String::from_utf8_lossy(data).trim_end_matches('\n')
                )
            };
            session.data(channel, reply.into_bytes())?;
            Ok(())
        }
    }

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
            let mut srv = TestServer;
            let _ = srv.run_on_socket(config, &socket).await;
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
            "test".into(),
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
        harness.wait_for("tester@host:~$ ").await;
        assert!(!harness.console().contains('\u{1b}'));
        assert!(harness.types().iter().any(|t| t == "status"));
        harness
            .input
            .send(SshInput::Data(b"iperf3 --version\n".to_vec()))
            .unwrap();
        harness.wait_for("iperf3 3.16").await;
        assert!(harness.console().contains("iperf3 --version"));
        harness.input.send(SshInput::Data(vec![3])).unwrap();
        harness.wait_for("^C").await;
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
            "unexpected: {:?}",
            last.message
        );
    }
}
