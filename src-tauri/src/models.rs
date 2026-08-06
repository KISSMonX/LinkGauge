use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestRequest {
    pub task_id: String,
    pub mode: String,
    /// 协议由 task_id 推断（udp-* 为 UDP），此字段仅兼容旧版前端，不再参与逻辑
    #[serde(default)]
    pub protocol: String,
    pub server_ip: String,
    #[serde(default)]
    pub local_ip: String,
    /// 服务端绑定 IP（仅服务端模式使用；空 = 绑定所有网卡）
    #[serde(default)]
    pub bind_ip: String,
    /// 界面语言（zh / en，空 = zh）：引擎日志按此语言输出
    #[serde(default)]
    pub locale: String,
    pub port: u16,
    pub duration: u64,
    pub parallel: u16,
    pub bandwidth: u64,
    /// TCP 报文长度（默认 128KB）
    pub packet_length: u32,
    /// UDP 报文长度（默认 1460 B，与 iperf3 的 DEFAULT_UDP_BLKSIZE 一致；最大 64KB）
    #[serde(default)]
    pub udp_packet_length: u64,
    pub interval: u64,
    /// 预热时间（秒，0 = 不预热，对应 iperf3 `-O`）：跳过前 N 秒的统计，
    /// 排除 TCP 慢启动影响；必须小于 duration
    #[serde(default)]
    pub omit_secs: u32,
    /// TCP 套接字缓冲区大小（KB，0 = 自动/默认，对应 iperf3 `-w`；仅客户端模式使用）
    #[serde(default)]
    pub window_kb: u32,
    /// 客户端数据流源端口（0 = 自动，对应 iperf3 `--cport`；第 i 条流绑定 cport+i）
    #[serde(default)]
    pub cport: u16,
    /// IP 协议族（0 = 自动，4 = 仅 IPv4，6 = 仅 IPv6；仅客户端模式使用）
    #[serde(default)]
    pub ip_version: u8,
    // —— iperf3 认证（对端以 --rsa-private-key-path + --authorized-users-path
    // 启动时必需）。全部 default，旧版前端不传时等价于不启用 ——
    /// 认证用户名；为空表示不启用认证
    #[serde(default)]
    pub auth_username: String,
    /// 认证密码（仅在内存中流转，前端不写入本地存储、不随配置导出）
    #[serde(default)]
    pub auth_password: String,
    /// 服务端 RSA 公钥文件路径，用于加密凭据
    #[serde(default)]
    pub auth_public_key_path: String,
    /// 对 iperf3 < 3.17 的服务端改用 PKCS#1 v1.5 填充（3.17+ 默认 OAEP）
    #[serde(default)]
    pub auth_pkcs1_padding: bool,
    // —— 服务端 iperf3 认证（要求客户端提供 --username/--password 凭据）——
    // 与客户端认证字段分开：服务端持有的是 RSA 私钥与授权用户文件，不涉及用户名/密码。
    // 全部 default，旧版前端不传时等价于不启用 ——
    /// 是否启用服务端认证（仅服务端模式使用）
    #[serde(default)]
    pub server_auth_enabled: bool,
    /// 服务端 RSA 私钥文件路径（PEM，用于解密客户端凭据，对应 --rsa-private-key-path）
    #[serde(default)]
    pub server_auth_private_key_path: String,
    /// 授权用户文件路径（对应 --authorized-users-path；格式见 riperf3 auth.rs：
    /// 每行 `用户名,sha256hex`，哈希为 sha256("{用户名}{密码}")，# 开头为注释）
    #[serde(default)]
    pub server_auth_users_path: String,
    /// 对 iperf3 < 3.17 的客户端改用 PKCS#1 v1.5 填充（3.17+ 默认 OAEP）
    #[serde(default)]
    pub server_auth_pkcs1_padding: bool,
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

/// 单个测试项目的历史数据（报告按测试项目分组输出：数据表 + 曲线）
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportItem {
    /// 测试项目名称（前端按界面语言传入，报告直接展示）
    pub label: String,
    /// 项目结束状态（success / failed / stopped）
    pub status: String,
    pub points: Vec<MetricPoint>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportRequest {
    pub format: String,
    #[serde(default)]
    pub save_path: Option<String>,
    /// 界面语言（zh / en，空 = zh）：HTML 报告按此语言生成
    #[serde(default)]
    pub locale: String,
    pub config: serde_json::Value,
    pub summary: serde_json::Value,
    /// 最近一次/当前展示的曲线数据（items 为空时的回退数据，兼容旧版调用）
    pub points: Vec<MetricPoint>,
    /// 按测试项目分组的数据（非空时报告按项目输出曲线与数据表）
    #[serde(default)]
    pub items: Vec<ReportItem>,
    pub logs: Vec<serde_json::Value>,
}
