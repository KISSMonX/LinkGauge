//! riperf3 引擎端到端冒烟测试（本地回环，无需外部服务）：
//! 1. 内嵌服务端 + 客户端跑一轮 TCP 测试，验证 on_interval 补丁的实时回调
//!    能收到逐秒数据，且测试正常完成；
//! 2. 空闲服务端收到中断信号后正常退出（runner.rs 服务端循环的退出条件）；
//! 3. 服务端启用认证后：携带正确凭据的客户端完成测试，无凭据客户端被拒绝
//!    （对应 runner.rs 服务端认证接线：私钥 + 授权用户文件）。

use riperf3::{ClientBuilder, ServerBuilder, Termination, TransportProtocol};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

/// 测试专用 RSA 密钥对（Node crypto 生成，仅用于本测试；公钥 SPKI PEM，
/// 私钥 PKCS#8 PEM —— 与 riperf3 的 parse_public_key_pem / parse_private_key_pem 对应）
const TEST_PRIVATE_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQCaKB/qpCQdT2hu
VV8aYqAbYORGJLJ+Gp8+A1TkyTHQRjdjNQQW7UvFG3zcs6VhpPRdNw6G+8AFEtLp
IlCdaEjVbURwLHJmRLd8oYq2xVxCgpQwoCXr7TcSMRt6mObYVcHk2Gm6YGSVlQwT
RyQnva0PvQmzR8Mu34Mtnv7nEffKKemgaGOhfINxXsogaaCouGa8OmFi8on9zaRm
dmBkgx82zF6cEsJWw6WCBokspU1434Qwue9C9cZKFDv9uAKysqGbZcF0Id0w6ZzK
8Y5MKqdCtPI5qxsb4eIRbqoOHzBdP0fcjov4vLQbGzy2jtwuDPKlKjo+yKQClgF9
gDls7AoDAgMBAAECggEAGWcv+JWl5zfqXfbZiNqPHGk3aiD//OyG00xElWhbj2u9
aC7DXBfXMEWwVMTnJZPDj7IpRbB/WxatWi0FGyX6jUPH+bLereBNsE8FMcs3bIpi
FdqThTD+WJQZEsyBi015JIH5IkpiKOLP2O6/PtabNIhn7Hrl53fTq7/orMv23lcP
jIdu2KzCr/CJwco5nlFtgIHoJoZ3Za3g4NNl1uEzAU9YNF9iqYvYhFC1RW0Muyuo
qCRhSJ83PXuu6ju+oT6fgLRee1KaZkwpRL9TKRnE+YWO2fedkYy9LB4Cez4evt71
V6S6tJMn4JoAieIpZY+ZAQDaZcJ0aQY/p2wBYPgz6QKBgQDSN+bSgSAfRAK3oOBg
uhd5gblcPaLVuUu1SPnacwBSqm4vSqbGFFjGubh6j6U8eoSgN/JCwa94tVHHLAzF
VUN+VHenGUS01wYEl9or0hbM07gCXIpfYpPw2sDgGsoZk2IN16JdaSjC/XjM77+Y
LIpTsE5yIlmHdhWEsGdYdHpi3QKBgQC7uqyMyTN8FA/KacKUwee4ZJPbgKte/cxw
jmprxGJ6D4QptvctUD2sGz/SDZzH1BzF72Jd5qmWoSajO/m+f8SZKMwA727yTKob
Eu0ZIBDJI2HIWAxJkTiLMW+VB0DorwGTh1nAVf7GYeXzkW0riKAqvjsoqRd87XgS
eIsrpeAiXwKBgQDJZo1aOCPSUJJZ42OUyDUdUE+KM/MB2BjUgin+RBeXG3mdDWRi
ebPkEKLRqTWhj6/o4DDWDEJU30KOE4HYvSuAqORJz0eoCinV1LZNLWZyrpSojohz
gjpCkxIeowvlHPLgWCtSWyGWTsmhbkCdRm7wZwWBC6/CvDs5eNhKQq3OcQKBgQCT
Fcuj8vCXwtAsc3i1PMflPUhrrwCWSJwphCv1i8TshcOzO1um8Tug4Si711aDarmw
i8Kyd8tf7ZtsQc2HaGwM5F4STYbL6S1OUSHbkbgVH9e5NONLsLBwvqcCSNCefp/p
ix7TB426uXGFyOeUOFPlqW6IiROSGiz9q9y+shROWQKBgBvoa6nao/NDN4at7q/w
0bdztWujANUkxNgk6Xr+IIXf+BuXnpJXEGu/4aclw0JCptUlqhNaHmc8VuVwWSJ1
lFSUcTsBkpek9JpyFXToJ7tkU4cwe62Z7T3XbYjmNa7dSQdicx8rF2OLnqlP9l2g
8HFnBp7Gnjxh5NP9nSIl6NOd
-----END PRIVATE KEY-----";

const TEST_PUBLIC_KEY_PEM: &str = "-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAmigf6qQkHU9oblVfGmKg
G2DkRiSyfhqfPgNU5Mkx0EY3YzUEFu1LxRt83LOlYaT0XTcOhvvABRLS6SJQnWhI
1W1EcCxyZkS3fKGKtsVcQoKUMKAl6+03EjEbepjm2FXB5NhpumBklZUME0ckJ72t
D70Js0fDLt+DLZ7+5xH3yinpoGhjoXyDcV7KIGmgqLhmvDphYvKJ/c2kZnZgZIMf
NsxenBLCVsOlggaJLKVNeN+EMLnvQvXGShQ7/bgCsrKhm2XBdCHdMOmcyvGOTCqn
QrTyOasbG+HiEW6qDh8wXT9H3I6L+Ly0Gxs8to7cLgzypSo6PsikApYBfYA5bOwK
AwIDAQAB
-----END PUBLIC KEY-----";

/// 授权用户文件内容：`testuser,sha256("{testuser}testpass")`（格式见 riperf3 auth.rs）
const TEST_USERS_FILE: &str =
    "testuser,6d30222cf5cb9f09b0175e1dbfbc0b6fef34fc08c2fdf02682e0c2450c9c7170\n";

/// 把测试密钥/用户文件写入临时目录，返回目录路径（测试结束时清理）
fn write_auth_fixtures() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "linkgauge-auth-fixtures-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("key.pem"), TEST_PRIVATE_KEY_PEM).unwrap();
    std::fs::write(dir.join("pub.pem"), TEST_PUBLIC_KEY_PEM).unwrap();
    std::fs::write(dir.join("users.csv"), TEST_USERS_FILE).unwrap();
    dir
}

#[tokio::test(flavor = "multi_thread")]
async fn client_server_roundtrip_with_live_intervals() {
    // 内嵌服务端：端口 0 表示临时端口，绑定后通过 local_addr 得知实际端口
    let (_server_tx, server_rx) = watch::channel(None);
    let server = ServerBuilder::new()
        .port(Some(0))
        .one_off(true)
        .json_output(true)
        .emit_output(false)
        .interrupt(server_rx)
        .build()
        .unwrap();
    let bound = server.bind().await.unwrap();
    let addr = bound.local_addr().unwrap();
    let server_task = tokio::spawn(async move { bound.run_once().await });

    // 客户端：2 秒测试，on_interval 回调收集实时区间数据
    let intervals: Arc<Mutex<Vec<f64>>> = Arc::new(Mutex::new(Vec::new()));
    let hook = intervals.clone();
    let client = ClientBuilder::new("127.0.0.1")
        .port(Some(addr.port()))
        .protocol(TransportProtocol::Tcp)
        .duration(2)
        .interval(1.0)
        .json_output(true)
        .emit_output(false)
        .on_interval(move |interval: &riperf3::json_report::Interval| {
            if !interval.sum.omitted {
                hook.lock().unwrap().push(interval.sum.bits_per_second);
            }
        })
        .build()
        .unwrap();
    let outcome = client.run().await.unwrap();

    assert_eq!(outcome.termination, Termination::Completed);
    assert!(
        !outcome.report.intervals.is_empty(),
        "最终报告应包含区间数据"
    );
    // 锁在独立作用域内取用：guard 不能活到下面的 await（clippy::await_holding_lock）
    {
        let live = intervals.lock().unwrap();
        assert!(!live.is_empty(), "on_interval 实时回调未收到任何区间数据");
        assert!(
            live.iter().all(|bps| *bps > 0.0),
            "区间带宽应大于 0：{live:?}"
        );
    }

    let server_outcome = server_task.await.unwrap().unwrap();
    assert_eq!(server_outcome.termination, Termination::Completed);
}

#[tokio::test(flavor = "multi_thread")]
async fn idle_server_stops_on_interrupt() {
    let (tx, rx) = watch::channel(None);
    let server = ServerBuilder::new()
        .port(Some(0))
        .one_off(true)
        .interrupt(rx)
        .build()
        .unwrap();
    let bound = server.bind().await.unwrap();
    let handle = tokio::spawn(async move { bound.run_once().await });
    // 空闲时发送中断：无测试运行，应返回 Aborted（runner.rs 据此正常退出循环）
    tx.send(Some("stop".into())).unwrap();
    let result = handle.await.unwrap();
    assert!(
        matches!(result, Err(riperf3::RiperfError::Aborted(_))),
        "空闲中断应返回 Aborted，实际：{result:?}"
    );
}

/// on_connect 补丁验证：客户端建立控制连接后，服务端与客户端的 on_connect
/// 回调都应触发，且服务端拿到的是对端（客户端）地址
#[tokio::test(flavor = "multi_thread")]
async fn on_connect_hooks_fire_with_peer_address() {
    let (_server_tx, server_rx) = watch::channel(None);
    let server_peer: Arc<Mutex<Option<std::net::SocketAddr>>> = Arc::new(Mutex::new(None));
    let server_hook = server_peer.clone();
    let server = ServerBuilder::new()
        .port(Some(0))
        .one_off(true)
        .json_output(true)
        .emit_output(false)
        .interrupt(server_rx)
        .on_connect(move |addr: std::net::SocketAddr| {
            *server_hook.lock().unwrap() = Some(addr);
        })
        .build()
        .unwrap();
    let bound = server.bind().await.unwrap();
    let addr = bound.local_addr().unwrap();
    let server_task = tokio::spawn(async move { bound.run_once().await });

    let client_local: Arc<Mutex<Option<u16>>> = Arc::new(Mutex::new(None));
    let client_hook = client_local.clone();
    let client = ClientBuilder::new("127.0.0.1")
        .port(Some(addr.port()))
        .protocol(TransportProtocol::Tcp)
        .duration(1)
        .interval(1.0)
        .json_output(true)
        .emit_output(false)
        .on_connect(move |addr: std::net::SocketAddr| {
            *client_hook.lock().unwrap() = Some(addr.port());
        })
        .build()
        .unwrap();
    client.run().await.unwrap();

    // 服务端 on_connect 收到的应是客户端地址（回环测试中为 127.0.0.1）
    let peer = *server_peer.lock().unwrap();
    eprintln!("DEBUG server on_connect peer: {peer:?}");
    assert!(peer.is_some(), "服务端 on_connect 未触发");
    assert!(
        peer.unwrap().ip().to_canonical().is_loopback(),
        "服务端 on_connect 应收到回环对端地址"
    );
    // 客户端 on_connect 收到的是本机控制连接端口（localPort 数据源）
    let local_port = *client_local.lock().unwrap();
    assert!(local_port.is_some(), "客户端 on_connect 未触发");

    let server_outcome = server_task.await.unwrap().unwrap();
    assert_eq!(server_outcome.termination, Termination::Completed);
}

/// 服务端启用认证（私钥 + 授权用户文件）：带正确凭据的客户端跑通测试，
/// 无凭据客户端被拒绝 —— 验证 runner.rs 的服务端认证接线与引擎握手
#[tokio::test(flavor = "multi_thread")]
async fn authenticated_client_succeeds_unauthenticated_denied() {
    let dir = write_auth_fixtures();
    let key_path = dir.join("key.pem");
    let users_path = dir.join("users.csv");
    let pub_path = dir.join("pub.pem");

    // —— 正例：客户端携带用户名/密码/公钥，测试正常完成 ——
    let (_server_tx, server_rx) = watch::channel(None);
    let server = ServerBuilder::new()
        .port(Some(0))
        .one_off(true)
        .json_output(true)
        .emit_output(false)
        .rsa_private_key_path(key_path.to_str().unwrap())
        .authorized_users_path(users_path.to_str().unwrap())
        .interrupt(server_rx)
        .build()
        .unwrap();
    let bound = server.bind().await.unwrap();
    let addr = bound.local_addr().unwrap();
    let server_task = tokio::spawn(async move { bound.run_once().await });

    let client = ClientBuilder::new("127.0.0.1")
        .port(Some(addr.port()))
        .protocol(TransportProtocol::Tcp)
        .duration(1)
        .interval(1.0)
        .json_output(true)
        .emit_output(false)
        .username("testuser")
        .password("testpass")
        .rsa_public_key_path(pub_path.to_str().unwrap())
        .build()
        .unwrap();
    let outcome = client.run().await.unwrap();
    assert_eq!(outcome.termination, Termination::Completed);
    let server_outcome = server_task.await.unwrap().unwrap();
    assert_eq!(server_outcome.termination, Termination::Completed);

    // —— 负例：不携带凭据的客户端必须被拒绝（认证服务端直接关闭控制连接） ——
    let (_server_tx, server_rx) = watch::channel(None);
    let server = ServerBuilder::new()
        .port(Some(0))
        .one_off(true)
        .json_output(true)
        .emit_output(false)
        .rsa_private_key_path(key_path.to_str().unwrap())
        .authorized_users_path(users_path.to_str().unwrap())
        .interrupt(server_rx)
        .build()
        .unwrap();
    let bound = server.bind().await.unwrap();
    let addr = bound.local_addr().unwrap();
    let server_task = tokio::spawn(async move { bound.run_once().await });

    let client = ClientBuilder::new("127.0.0.1")
        .port(Some(addr.port()))
        .protocol(TransportProtocol::Tcp)
        .duration(1)
        .interval(1.0)
        .json_output(true)
        .emit_output(false)
        .build()
        .unwrap();
    assert!(
        client.run().await.is_err(),
        "无凭据客户端应被认证服务端拒绝"
    );
    // 服务端同样以错误结束（拒绝后不再接受数据流，run_once 返回错误）
    assert!(server_task.await.unwrap().is_err());

    let _ = std::fs::remove_dir_all(&dir);
}

/// 预热（-O）与套接字缓冲（-w）端到端：预热期的区间以 omitted 标记实时回调
/// （runner.rs 据此跳过图表/日志），最终报告不含预热段；-w 不影响测试完成
#[tokio::test(flavor = "multi_thread")]
async fn omit_and_window_params_work_end_to_end() {
    let (_server_tx, server_rx) = watch::channel(None);
    let server = ServerBuilder::new()
        .port(Some(0))
        .one_off(true)
        .json_output(true)
        .emit_output(false)
        .interrupt(server_rx)
        .build()
        .unwrap();
    let bound = server.bind().await.unwrap();
    let addr = bound.local_addr().unwrap();
    let server_task = tokio::spawn(async move { bound.run_once().await });

    // 3 秒测试 + 1 秒预热：前 1 秒区间应被标记 omitted
    let flags: Arc<Mutex<Vec<bool>>> = Arc::new(Mutex::new(Vec::new()));
    let hook = flags.clone();
    let client = ClientBuilder::new("127.0.0.1")
        .port(Some(addr.port()))
        .protocol(TransportProtocol::Tcp)
        .duration(3)
        .interval(1.0)
        .omit(1)
        .window(64 * 1024) // 64 KB 套接字缓冲（-w 64K）
        .json_output(true)
        .emit_output(false)
        .on_interval(move |interval: &riperf3::json_report::Interval| {
            hook.lock().unwrap().push(interval.sum.omitted);
        })
        .build()
        .unwrap();
    let outcome = client.run().await.unwrap();
    assert_eq!(outcome.termination, Termination::Completed);
    {
        let seen = flags.lock().unwrap();
        assert!(
            seen.contains(&true),
            "预热期区间应以 omitted 标记回调：{seen:?}"
        );
        assert!(
            seen.contains(&false),
            "预热结束后应有正常统计区间：{seen:?}"
        );
    }
    // 与 iperf3 一致：报告保留预热区间行（omitted 标记），但汇总排除预热段
    let report = &outcome.report;
    assert!(
        report.intervals.iter().any(|interval| interval.sum.omitted),
        "报告应保留预热区间行（omitted 标记）"
    );
    let measured = report
        .intervals
        .iter()
        .filter(|interval| !interval.sum.omitted)
        .count();
    assert!(
        measured >= 2,
        "预热结束后应有正常统计区间，实际 {measured} 个"
    );
    let sum = outcome.report.end.sum_sent.as_ref().expect("应有发送汇总");
    let measured_bytes: u64 = report
        .intervals
        .iter()
        .filter(|interval| !interval.sum.omitted)
        .map(|interval| interval.sum.bytes)
        .sum();
    // 汇总窗口只覆盖预热后的测量段（与 iperf3 的 [SUM] 行一致）；
    // 曾回归为 seconds=3（全程），导致聚合带宽被低估（vendor 补丁修复）
    assert!(
        (sum.seconds - 2.0).abs() < 0.01,
        "汇总秒数应排除预热段（≈2s），实际 {}",
        sum.seconds
    );
    assert!(
        (sum.start - 1.0).abs() < 0.01,
        "汇总起点应为预热结束（≈1s），实际 {}",
        sum.start
    );
    assert!(
        (sum.end - 3.0).abs() < 0.01,
        "汇总终点应为测试全程结束（≈3s），实际 {}",
        sum.end
    );
    assert_eq!(
        sum.bytes, measured_bytes,
        "汇总字节数应与非预热区间一致（不含预热）"
    );

    let server_outcome = server_task.await.unwrap().unwrap();
    assert_eq!(server_outcome.termination, Termination::Completed);
}

/// 客户端源端口（--cport）与 IP 协议族（-4）端到端：单流时数据流绑定
/// cport；显式 IPv4 强制地址族解析（runner.rs 的 cport/ip_version 接线）
#[tokio::test(flavor = "multi_thread")]
async fn cport_and_ip_version_work_end_to_end() {
    // 先探测一个空闲端口作为 cport，避免固定端口被占用导致测试偶发失败
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let cport = probe.local_addr().unwrap().port();
    drop(probe);

    let (_server_tx, server_rx) = watch::channel(None);
    let server = ServerBuilder::new()
        .port(Some(0))
        .one_off(true)
        .json_output(true)
        .emit_output(false)
        .interrupt(server_rx)
        .build()
        .unwrap();
    let bound = server.bind().await.unwrap();
    let addr = bound.local_addr().unwrap();
    let server_task = tokio::spawn(async move { bound.run_once().await });

    let client = ClientBuilder::new("127.0.0.1")
        .port(Some(addr.port()))
        .protocol(TransportProtocol::Tcp)
        .duration(1)
        .interval(1.0)
        .cport(cport)
        .ip_version(4)
        .json_output(true)
        .emit_output(false)
        .build()
        .unwrap();
    let outcome = client.run().await.unwrap();
    assert_eq!(outcome.termination, Termination::Completed);

    let server_outcome = server_task.await.unwrap().unwrap();
    assert_eq!(server_outcome.termination, Termination::Completed);
}

/// 服务端防护参数（--idle-timeout / --server-max-duration / --server-bitrate-limit）
/// 端到端：全部设置后正常测试仍应完成（runner.rs 的防护参数接线）
#[tokio::test(flavor = "multi_thread")]
async fn server_protection_params_allow_normal_test() {
    let (_server_tx, server_rx) = watch::channel(None);
    let server = ServerBuilder::new()
        .port(Some(0))
        .one_off(true)
        .json_output(true)
        .emit_output(false)
        .idle_timeout(60)
        .server_max_duration(60)
        .server_bitrate_limit(1_000_000_000_000) // 1 Tbps，回环不可能触发
        .interrupt(server_rx)
        .build()
        .unwrap();
    let bound = server.bind().await.unwrap();
    let addr = bound.local_addr().unwrap();
    let server_task = tokio::spawn(async move { bound.run_once().await });

    let client = ClientBuilder::new("127.0.0.1")
        .port(Some(addr.port()))
        .protocol(TransportProtocol::Tcp)
        .duration(1)
        .interval(1.0)
        .json_output(true)
        .emit_output(false)
        .build()
        .unwrap();
    let outcome = client.run().await.unwrap();
    assert_eq!(outcome.termination, Termination::Completed);

    let server_outcome = server_task.await.unwrap().unwrap();
    assert_eq!(server_outcome.termination, Termination::Completed);
}

/// 空闲超时语义（runner.rs 循环依赖它退出）：one_off 服务端在 N 秒无客户端
/// 连接后，run_once 返回 Aborted("idle timeout")
#[tokio::test(flavor = "multi_thread")]
async fn idle_timeout_aborts_with_specific_message() {
    let (_server_tx, server_rx) = watch::channel(None);
    let server = ServerBuilder::new()
        .port(Some(0))
        .one_off(true)
        .idle_timeout(1)
        .interrupt(server_rx)
        .build()
        .unwrap();
    let bound = server.bind().await.unwrap();
    let started = std::time::Instant::now();
    let result = bound.run_once().await;
    assert!(
        matches!(&result, Err(riperf3::RiperfError::Aborted(msg)) if msg == "idle timeout"),
        "空闲超时应返回 Aborted(\"idle timeout\")，实际：{result:?}"
    );
    assert!(started.elapsed().as_secs() < 10, "空闲超时应在一秒左右触发");
}

/// --get-server-output 端到端：文本模式服务端的汇总随测试结果返回，
/// 客户端报告的 server_output_text 应包含服务端视角的区间/汇总行
/// （runner.rs 将其写入测试日志并广播）
#[tokio::test(flavor = "multi_thread")]
async fn get_server_output_captures_server_text_report() {
    let (_server_tx, server_rx) = watch::channel(None);
    let server = ServerBuilder::new()
        .port(Some(0))
        .one_off(true)
        .json_output(false) // 文本模式：服务端才为客户端捕获文本输出
        .emit_output(false)
        .interrupt(server_rx)
        .build()
        .unwrap();
    let bound = server.bind().await.unwrap();
    let addr = bound.local_addr().unwrap();
    let server_task = tokio::spawn(async move { bound.run_once().await });

    let client = ClientBuilder::new("127.0.0.1")
        .port(Some(addr.port()))
        .protocol(TransportProtocol::Tcp)
        .duration(1)
        .interval(1.0)
        .get_server_output(true)
        .json_output(true)
        .emit_output(false)
        .build()
        .unwrap();
    let outcome = client.run().await.unwrap();
    assert_eq!(outcome.termination, Termination::Completed);
    let server_text = outcome
        .report
        .server_output_text
        .filter(|t| !t.trim().is_empty())
        .expect("get_server_output 应带回服务端文本");
    assert!(
        server_text.contains("sec"),
        "服务端文本应包含区间/汇总行，实际：{server_text}"
    );

    let server_outcome = server_task.await.unwrap().unwrap();
    assert_eq!(server_outcome.termination, Termination::Completed);
}

/// 按量测试（-n，MB → 字节）与 DSCP（--dscp）端到端：
/// 传输 2 MB 即结束（时长 60 被 -n 覆盖），汇总字节数 ≥ 2MB；DSCP 不破坏流程
#[tokio::test(flavor = "multi_thread")]
async fn byte_limited_transfer_and_dscp_work_end_to_end() {
    let (_server_tx, server_rx) = watch::channel(None);
    let server = ServerBuilder::new()
        .port(Some(0))
        .one_off(true)
        .json_output(true)
        .emit_output(false)
        .interrupt(server_rx)
        .build()
        .unwrap();
    let bound = server.bind().await.unwrap();
    let addr = bound.local_addr().unwrap();
    let server_task = tokio::spawn(async move { bound.run_once().await });

    // 2 MB 传输量 + EF (46) DSCP：时长被忽略，传完即结束
    let client = ClientBuilder::new("127.0.0.1")
        .port(Some(addr.port()))
        .protocol(TransportProtocol::Tcp)
        .duration(60)
        .interval(1.0)
        .bytes(2_000_000)
        .dscp("46")
        .json_output(true)
        .emit_output(false)
        .build()
        .unwrap();
    let started = std::time::Instant::now();
    let outcome = client.run().await.unwrap();
    assert_eq!(outcome.termination, Termination::Completed);
    assert!(
        started.elapsed().as_secs() < 10,
        "2 MB 回环传输应远快于 10 秒（时长 60 应被 -n 覆盖）"
    );
    let sum = outcome.report.end.sum_sent.as_ref().expect("应有发送汇总");
    assert!(
        sum.bytes >= 2_000_000,
        "按量模式应至少传输 2 MB，实际 {}",
        sum.bytes
    );

    let server_outcome = server_task.await.unwrap().unwrap();
    assert_eq!(server_outcome.termination, Termination::Completed);
}

/// UDP 禁止分片（--dont-fragment）端到端：IPv4 下设置 DF 标志，
/// 测试须正常完成（Windows / Unix 均有实现，runner.rs 的 DF 接线）。
/// 注意：UDP 测试不能用 port(Some(0))——控制监听与 UDP demux 分别拿
/// 到不同的临时端口，魔数会打空（引擎限制，真实应用总是固定端口）
#[tokio::test(flavor = "multi_thread")]
async fn dont_fragment_works_end_to_end() {
    // 探测一个空闲端口给服务端使用（UDP 下控制与 demux 必须同端口）
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);

    let (_server_tx, server_rx) = watch::channel(None);
    let server = ServerBuilder::new()
        .port(Some(port))
        .one_off(true)
        .json_output(true)
        .emit_output(false)
        .interrupt(server_rx)
        .build()
        .unwrap();
    let bound = server.bind().await.unwrap();
    let addr = bound.local_addr().unwrap();
    let server_task = tokio::spawn(async move { bound.run_once().await });

    let client = ClientBuilder::new("127.0.0.1")
        .port(Some(addr.port()))
        .protocol(TransportProtocol::Udp)
        .duration(1)
        .interval(1.0)
        .bandwidth(100_000_000) // 100 Mbps，避免引擎的 1 Mibit/s 默认限速
        .dont_fragment(true)
        .json_output(true)
        .emit_output(false)
        .build()
        .unwrap();
    let outcome = client.run().await.unwrap();
    assert_eq!(outcome.termination, Termination::Completed);

    let server_outcome = server_task.await.unwrap().unwrap();
    assert_eq!(server_outcome.termination, Termination::Completed);
}

/// 拥塞控制（-C）端到端：仅 Unix 平台（引擎在 Windows 的 build() 直接
/// 拒绝，LinkGauge validate 同样拦截），Linux 上 cubic 真实生效
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn congestion_control_works_end_to_end() {
    let (_server_tx, server_rx) = watch::channel(None);
    let server = ServerBuilder::new()
        .port(Some(0))
        .one_off(true)
        .json_output(true)
        .emit_output(false)
        .interrupt(server_rx)
        .build()
        .unwrap();
    let bound = server.bind().await.unwrap();
    let addr = bound.local_addr().unwrap();
    let server_task = tokio::spawn(async move { bound.run_once().await });

    let client = ClientBuilder::new("127.0.0.1")
        .port(Some(addr.port()))
        .protocol(TransportProtocol::Tcp)
        .duration(1)
        .interval(1.0)
        .congestion("cubic")
        .json_output(true)
        .emit_output(false)
        .build()
        .unwrap();
    let outcome = client.run().await.unwrap();
    assert_eq!(outcome.termination, Termination::Completed);

    let server_outcome = server_task.await.unwrap().unwrap();
    assert_eq!(server_outcome.termination, Termination::Completed);
}

/// 服务端统计采样间隔（-i，本地补丁）端到端：服务端 interval(2.0) 时，
/// 5 秒测试的服务端 on_interval 回调应远少于 1s 节拍的 5 次
/// （2 个整区间 + 1 个尾区间 = 至多 3 次；runner.rs 据此跟随界面设置）
#[tokio::test(flavor = "multi_thread")]
async fn server_interval_controls_sampling_cadence() {
    let (_server_tx, server_rx) = watch::channel(None);
    let server_intervals: Arc<Mutex<Vec<f64>>> = Arc::new(Mutex::new(Vec::new()));
    let hook = server_intervals.clone();
    let server = ServerBuilder::new()
        .port(Some(0))
        .one_off(true)
        .json_output(true)
        .emit_output(false)
        .interval(2.0)
        .interrupt(server_rx)
        .on_interval(move |interval: &riperf3::json_report::Interval| {
            if !interval.sum.omitted {
                hook.lock().unwrap().push(interval.sum.end);
            }
        })
        .build()
        .unwrap();
    let bound = server.bind().await.unwrap();
    let addr = bound.local_addr().unwrap();
    let server_task = tokio::spawn(async move { bound.run_once().await });

    let client = ClientBuilder::new("127.0.0.1")
        .port(Some(addr.port()))
        .protocol(TransportProtocol::Tcp)
        .duration(5)
        .interval(1.0)
        .json_output(true)
        .emit_output(false)
        .build()
        .unwrap();
    let outcome = client.run().await.unwrap();
    assert_eq!(outcome.termination, Termination::Completed);

    let server_outcome = server_task.await.unwrap().unwrap();
    assert_eq!(server_outcome.termination, Termination::Completed);
    let ends = server_intervals.lock().unwrap();
    assert!(!ends.is_empty(), "服务端应收到至少一个统计区间");
    assert!(
        ends.len() <= 3,
        "2s 采样间隔下 5 秒测试至多 3 个区间（2 整 + 1 尾），实际 {} 个：{ends:?}",
        ends.len()
    );
}
