//! 测试请求参数校验，从 runner.rs 拆分出独立模块以便单测。
//!
//! 校验逻辑集中在一处，Validator 接受所有参数后返回第一个不合规的原因。

use crate::client;
use crate::error::ValidationError;
use crate::models::TestRequest;

/// 完整校验客户端 / 服务端测试请求参数。
/// 返回 `Ok(())` 通过，或 `Err(ValidationError)` 指出第一个违规字段。
pub(crate) fn validate(request: &TestRequest) -> Result<(), ValidationError> {
    // 服务端模式不需要持续时间（duration 恒为 0）；按量测试（-n/-k）忽略时长。
    let transfer_mode = client::effective_transfer_mode(request);
    if request.mode != "server" && transfer_mode == "time" && request.duration == 0 {
        return Err(ValidationError::DurationRequired);
    }
    if request.interval == 0 {
        return Err(ValidationError::IntervalRequired);
    }
    if request.server_ip.trim().is_empty() && request.mode != "server" {
        return Err(ValidationError::ServerIpRequired);
    }
    if request.port == 0 {
        return Err(ValidationError::InvalidPort);
    }
    // 服务端认证依赖私钥与用户文件两个路径，缺一不可
    if request.mode == "server"
        && request.server_auth_enabled
        && (request.server_auth_private_key_path.trim().is_empty()
            || request.server_auth_users_path.trim().is_empty())
    {
        return Err(ValidationError::ServerAuthIncomplete);
    }
    // 预热与按量测试互斥
    if request.omit_secs > 0 {
        if transfer_mode != "time" {
            return Err(ValidationError::OmitOnlyTimeMode);
        }
        if u64::from(request.omit_secs) >= request.duration {
            return Err(ValidationError::OmitTooLong);
        }
    }
    // 套接字缓冲区上限
    if request.window_kb > 16384 {
        return Err(ValidationError::WindowTooLarge {
            requested_kb: request.window_kb,
        });
    }
    // IP 协议族只接受 0（自动）/ 4 / 6
    if !matches!(request.ip_version, 0 | 4 | 6) {
        return Err(ValidationError::InvalidIpVersion {
            value: request.ip_version,
        });
    }
    // 服务端防护参数范围
    if request.server_idle_timeout > 86400 {
        return Err(ValidationError::ServerIdleTimeoutTooLarge);
    }
    if request.server_max_duration > 86400 {
        return Err(ValidationError::ServerMaxDurationTooLarge);
    }
    if request.server_bitrate_limit_mbps > 1_000_000 {
        return Err(ValidationError::ServerBitrateLimitTooLarge);
    }
    // 结束条件：time / bytes / blocks
    match transfer_mode {
        "time" => {}
        "bytes" | "blocks" if request.transfer_amount > 0 => {}
        "bytes" | "blocks" => return Err(ValidationError::TransferAmountRequired),
        _ => return Err(ValidationError::InvalidTransferMode),
    }
    // DSCP 范围 0-63
    if request.dscp > 63 {
        return Err(ValidationError::DscpOutOfRange);
    }
    // 拥塞控制仅 Linux/FreeBSD 支持
    #[cfg(not(unix))]
    if !request.congestion_algo.trim().is_empty() {
        return Err(ValidationError::CongestionAlgoNotSupported);
    }
    Ok(())
}
