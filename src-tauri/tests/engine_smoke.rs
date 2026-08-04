//! riperf3 引擎端到端冒烟测试（本地回环，无需外部服务）：
//! 1. 内嵌服务端 + 客户端跑一轮 TCP 测试，验证 on_interval 补丁的实时回调
//!    能收到逐秒数据，且测试正常完成；
//! 2. 空闲服务端收到中断信号后正常退出（runner.rs 服务端循环的退出条件）。

use riperf3::{ClientBuilder, ServerBuilder, Termination, TransportProtocol};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

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
