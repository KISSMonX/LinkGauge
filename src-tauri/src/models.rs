use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestRequest {
    pub task_id: String,
    pub mode: String,
    pub protocol: String,
    pub server_ip: String,
    #[serde(default)]
    pub local_ip: String,
    pub port: u16,
    pub duration: u64,
    pub parallel: u16,
    pub bandwidth: u64,
    pub packet_length: u32,
    pub interval: u64,
    pub iperf_path: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricPoint {
    pub second: i64,
    pub bandwidth_mbps: f64,
    pub transfer_mb: f64,
    pub jitter_ms: f64,
    pub loss_percent: f64,
    pub retransmits: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestEvent {
    pub session_id: String,
    pub task_id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metric: Option<MetricPoint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_path: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkInfo {
    pub ip: String,
    pub mac: String,
    pub hostname: String,
    pub interface_name: String,
    pub speed_mbps: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InterfaceInfo {
    pub ip: String,
    pub mac: String,
    pub interface_name: String,
    /// 网卡链路速率（Mbps），0 表示未知
    pub speed_mbps: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IperfRuntimeInfo {
    pub available: bool,
    pub bundled: bool,
    pub path: String,
    pub version: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportRequest {
    pub format: String,
    pub config: serde_json::Value,
    pub summary: serde_json::Value,
    pub points: Vec<MetricPoint>,
    pub logs: Vec<serde_json::Value>,
}
