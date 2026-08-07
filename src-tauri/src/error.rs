//! 结构化错误类型：替代裸 String，使调用方可以程序化区分失败原因，
//! 而不再依赖文案匹配。用户可见的消息通过 message() 按 locale 生成。

use std::fmt;

/// 测试请求参数校验失败的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    /// 客户端模式下 duration 为 0（且不是按量模式）
    DurationRequired,
    /// 输出周期为 0
    IntervalRequired,
    /// 非服务端模式且服务端地址为空
    ServerIpRequired,
    /// 端口为 0
    InvalidPort,
    /// 服务端认证启用但缺少私钥或用户文件
    ServerAuthIncomplete,
    /// 预热仅支持按时长模式
    OmitOnlyTimeMode,
    /// 预热时间 >= 测试时长
    OmitTooLong,
    /// 套接字缓冲区超过 16 MB
    WindowTooLarge { requested_kb: u32 },
    /// IP 协议族不是 0 / 4 / 6
    InvalidIpVersion { value: u8 },
    /// 服务端空闲超时 > 86400
    ServerIdleTimeoutTooLarge,
    /// 单测最大时长 > 86400
    ServerMaxDurationTooLarge,
    /// 服务端带宽上限 > 1_000_000 Mbps
    ServerBitrateLimitTooLarge,
    /// 按量模式但传输量为 0
    TransferAmountRequired,
    /// 结束条件不是 time / bytes / blocks
    InvalidTransferMode,
    /// DSCP > 63
    DscpOutOfRange,
    /// 非 Unix 平台设置了拥塞控制算法
    CongestionAlgoNotSupported,
}

impl ValidationError {
    /// 按当前界面语言返回用户可读的错误描述。
    /// locale 为空或非 "en" 时默认中文。
    pub fn message(&self, locale: &str) -> &str {
        let en = locale == "en";
        match self {
            Self::DurationRequired => {
                if en { "Duration must be greater than 0" } else { "持续时间必须大于 0" }
            }
            Self::IntervalRequired => {
                if en { "Output interval must be greater than 0" } else { "输出周期必须大于 0" }
            }
            Self::ServerIpRequired => {
                if en { "Server address cannot be empty" } else { "服务端地址不能为空" }
            }
            Self::InvalidPort => {
                if en { "Invalid port" } else { "端口无效" }
            }
            Self::ServerAuthIncomplete => {
                if en {
                    "When authentication is enabled, both the RSA private key and authorized-users file paths are required"
                } else {
                    "启用认证后，RSA 私钥与授权用户文件路径均不能为空"
                }
            }
            Self::OmitOnlyTimeMode => {
                if en { "Warm-up (-O) is only supported in time-based mode" } else { "预热（-O）仅支持按时长模式" }
            }
            Self::OmitTooLong => {
                if en { "Warm-up time must be shorter than the test duration" } else { "预热时间必须小于测试时长" }
            }
            Self::WindowTooLarge { .. } => {
                if en { "Socket buffer cannot exceed 16 MB" } else { "套接字缓冲区不能超过 16MB" }
            }
            Self::InvalidIpVersion { .. } => {
                if en { "IP version must be 0 (auto), 4, or 6" } else { "IP 协议族只能是 0（自动）、4 或 6" }
            }
            Self::ServerIdleTimeoutTooLarge => {
                if en { "Server idle timeout cannot exceed 86400 seconds" } else { "服务端空闲超时不能超过 86400 秒" }
            }
            Self::ServerMaxDurationTooLarge => {
                if en { "Maximum test duration cannot exceed 86400 seconds" } else { "单次测试最大时长不能超过 86400 秒" }
            }
            Self::ServerBitrateLimitTooLarge => {
                if en { "Server bitrate limit cannot exceed 1,000,000 Mbps" } else { "服务端带宽上限不能超过 1000000 Mbps" }
            }
            Self::TransferAmountRequired => {
                if en { "Transfer amount must be greater than 0" } else { "按量测试的传输量必须大于 0" }
            }
            Self::InvalidTransferMode => {
                if en { "Transfer mode must be time, bytes, or blocks" } else { "测试结束条件只能是 time / bytes / blocks" }
            }
            Self::DscpOutOfRange => {
                if en { "DSCP value must be between 0 and 63" } else { "DSCP 值应在 0-63 之间" }
            }
            Self::CongestionAlgoNotSupported => {
                if en { "Congestion control algorithm (-C) is only supported on Linux/FreeBSD" } else { "拥塞控制算法（-C）仅支持 Linux/FreeBSD" }
            }
        }
    }
}

impl fmt::Display for ValidationError {
    /// Display 默认中文，向后兼容。新调用请使用 message(locale)。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message("zh"))
    }
}

impl From<ValidationError> for String {
    fn from(e: ValidationError) -> Self {
        e.to_string()
    }
}
