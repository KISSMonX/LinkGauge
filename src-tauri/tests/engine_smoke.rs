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
    let live = intervals.lock().unwrap();
    assert!(!live.is_empty(), "on_interval 实时回调未收到任何区间数据");
    assert!(
        live.iter().all(|bps| *bps > 0.0),
        "区间带宽应大于 0：{live:?}"
    );
    drop(live);

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
