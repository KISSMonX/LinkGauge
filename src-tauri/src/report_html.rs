//! HTML 报告渲染：将结构化数据转为自包含 HTML 页面（含内联 CSS + SVG 曲线）。
//!
//! 从 report.rs 拆分，使 HTML 模板可独立阅读与调整样式。

use crate::models::{MetricPoint, ReportItem, ReportRequest};
use chrono::Local;

/// 渲染完整的自包含 HTML 测试报告（CSS 内联，离线可打开）
pub(crate) fn render_html(request: &ReportRequest) -> String {
    let logs = request
        .logs
        .iter()
        .map(|l| format!("<div>{}</div>", escape_html(&log_line(l))))
        .collect::<String>();
    let is_en = request.locale == "en";
    let config_section = config_section_html(request, is_en);
    let stats_section = stats_section_html(request, is_en);
    let data_section = if request.items.is_empty() {
        format!(
            "<section><h2>{}</h2><table>{}{}</table></section>",
            if is_en { "Test Data" } else { "测试数据" },
            table_head(is_en),
            table_rows(&request.points)
        )
    } else {
        request
            .items
            .iter()
            .map(|item| item_section_html(item, is_en))
            .collect::<String>()
    };
    if is_en {
        format!(
            r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><title>LinkGauge Test Report</title><style>@page{{size:A4;margin:12mm}}body{{font:14px Arial,'Microsoft YaHei';max-width:1000px;margin:35px auto;color:#172033}}h1{{color:#096edc}}section{{border:1px solid #dce2ea;border-radius:8px;padding:18px;margin:16px 0;break-inside:avoid}}table{{width:100%;border-collapse:collapse}}th,td{{padding:9px;border-bottom:1px solid #e5e9ef;text-align:right}}th:first-child,td:first-child{{text-align:left}}table.kv th,table.kv td{{text-align:left}}.logs{{font:12px Consolas;max-height:360px;overflow:auto}}@media print{{body{{max-width:none;margin:0}}.logs{{max-height:none;overflow:visible}}}}</style></head><body><h1>LinkGauge Test Report</h1><p>Generated: {}</p>{}{}{}<section><h2>Run Logs</h2><div class="logs">{}</div></section></body></html>"#,
            Local::now().format("%Y-%m-%d %H:%M:%S"),
            config_section,
            stats_section,
            data_section,
            logs
        )
    } else {
        format!(
            r#"<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><title>LinkGauge 测试报告</title><style>@page{{size:A4;margin:12mm}}body{{font:14px Arial,'Microsoft YaHei';max-width:1000px;margin:35px auto;color:#172033}}h1{{color:#096edc}}section{{border:1px solid #dce2ea;border-radius:8px;padding:18px;margin:16px 0;break-inside:avoid}}table{{width:100%;border-collapse:collapse}}th,td{{padding:9px;border-bottom:1px solid #e5e9ef;text-align:right}}th:first-child,td:first-child{{text-align:left}}table.kv th,table.kv td{{text-align:left}}.logs{{font:12px Consolas;max-height:360px;overflow:auto}}@media print{{body{{max-width:none;margin:0}}.logs{{max-height:none;overflow:visible}}}}</style></head><body><h1>LinkGauge 测试报告</h1><p>生成时间：{}</p>{}{}{}<section><h2>执行日志</h2><div class="logs">{}</div></section></body></html>"#,
            Local::now().format("%Y-%m-%d %H:%M:%S"),
            config_section,
            stats_section,
            data_section,
            logs
        )
    }
}

pub(crate) fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ---------------------------------------------------------------------------
// 配置 / 统计表格常量
// ---------------------------------------------------------------------------

const CONFIG_ORDER: [&str; 9] = [
    "mode",
    "serverIp",
    "port",
    "duration",
    "parallel",
    "bandwidth",
    "packetLength",
    "udpPacketLength",
    "interval",
];
const CONFIG_LABELS: [(&str, &str, &str); 9] = [
    ("mode", "测试模式", "Mode"),
    ("serverIp", "服务端地址", "Server IP"),
    ("port", "端口", "Port"),
    ("duration", "测试时长（秒）", "Duration (s)"),
    ("parallel", "并发流数", "Parallel streams"),
    ("bandwidth", "带宽限制（Mbps）", "Bandwidth limit (Mbps)"),
    (
        "packetLength",
        "TCP 报文长度（字节）",
        "TCP packet length (B)",
    ),
    (
        "udpPacketLength",
        "UDP 报文长度（字节）",
        "UDP packet length (B)",
    ),
    ("interval", "采样间隔（秒）", "Sampling interval (s)"),
];

const STATS_ORDER: [&str; 11] = [
    "startedAt",
    "completed",
    "total",
    "averageBandwidth",
    "maxBandwidth",
    "minBandwidth",
    "totalTransferMb",
    "pingAverage",
    "lossPercent",
    "jitterMs",
    "logPaths",
];
const STATS_LABELS: [(&str, &str, &str); 11] = [
    ("startedAt", "测试时间", "Started at"),
    ("completed", "已完成项目数", "Completed items"),
    ("total", "总项目数", "Total items"),
    (
        "averageBandwidth",
        "平均带宽（Mbps）",
        "Average bandwidth (Mbps)",
    ),
    ("maxBandwidth", "最大带宽（Mbps）", "Max bandwidth (Mbps)"),
    ("minBandwidth", "最小带宽（Mbps）", "Min bandwidth (Mbps)"),
    ("totalTransferMb", "总传输量（MB）", "Total transfer (MB)"),
    ("pingAverage", "平均 Ping（ms）", "Average ping (ms)"),
    ("lossPercent", "丢包率（%）", "Packet loss (%)"),
    ("jitterMs", "抖动（ms）", "Jitter (ms)"),
    ("logPaths", "日志文件", "Log files"),
];

fn config_section_html(request: &ReportRequest, is_en: bool) -> String {
    format!(
        "<section><h2>{}</h2>{}</section>",
        if is_en {
            "Test Configuration"
        } else {
            "测试配置"
        },
        kv_table(
            &request.config,
            &CONFIG_ORDER,
            &CONFIG_LABELS,
            is_en,
            if is_en { "Parameter" } else { "参数" }
        )
    )
}

fn stats_section_html(request: &ReportRequest, is_en: bool) -> String {
    format!(
        "<section><h2>{}</h2>{}</section>",
        if is_en { "Statistics" } else { "统计结果" },
        kv_table(
            &request.summary,
            &STATS_ORDER,
            &STATS_LABELS,
            is_en,
            if is_en { "Metric" } else { "统计项" }
        )
    )
}

fn item_section_html(item: &ReportItem, is_en: bool) -> String {
    let status = status_word(&item.status, is_en);
    let heading = if is_en {
        format!("Test Item: {} ({status})", escape_html(&item.label))
    } else {
        format!("测试项：{}（{status}）", escape_html(&item.label))
    };
    format!(
        "<section><h2>{heading}</h2>{}<table>{}{}</table></section>",
        svg_curve(&item.points, is_en),
        table_head(is_en),
        table_rows(&item.points)
    )
}

fn kv_table(
    obj: &serde_json::Value,
    order: &[&str],
    labels: &[(&str, &str, &str)],
    is_en: bool,
    head1: &str,
) -> String {
    let rows = order
        .iter()
        .filter_map(|k| {
            obj.get(*k).map(|v| {
                let label = labels
                    .iter()
                    .find(|(key, _, _)| key == k)
                    .map(|(_, zh, en)| if is_en { *en } else { *zh })
                    .unwrap_or(k);
                format!(
                    "<tr><td>{}</td><td>{}</td></tr>",
                    escape_html(label),
                    cell_html(k, v, is_en)
                )
            })
        })
        .collect::<String>();
    format!(
        "<table class=\"kv\"><thead><tr><th>{head1}</th><th>{}</th></tr></thead>{rows}</table>",
        if is_en { "Value" } else { "值" }
    )
}

fn cell_html(key: &str, v: &serde_json::Value, is_en: bool) -> String {
    match v {
        serde_json::Value::Array(arr) => {
            let joined = arr
                .iter()
                .filter_map(|x| x.as_str())
                .map(escape_html)
                .collect::<Vec<_>>()
                .join("<br>");
            if joined.is_empty() {
                "—".to_string()
            } else {
                joined
            }
        }
        _ => escape_html(&readable_value(key, v, is_en)),
    }
}

fn readable_value(key: &str, v: &serde_json::Value, is_en: bool) -> String {
    match v {
        serde_json::Value::String(s) => match (key, s.as_str()) {
            ("mode", "client") => (if is_en { "Client" } else { "客户端" }).to_string(),
            ("mode", "server") => (if is_en { "Server" } else { "服务端" }).to_string(),
            _ => s.clone(),
        },
        serde_json::Value::Number(n) => n
            .as_f64()
            .map(|f| {
                if f.fract() == 0.0 {
                    format!("{f:.0}")
                } else {
                    format!("{f:.2}")
                }
            })
            .unwrap_or_else(|| v.to_string()),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => "—".to_string(),
        _ => v.to_string(),
    }
}

fn log_line(l: &serde_json::Value) -> String {
    let get = |k: &str| l.get(k).and_then(|v| v.as_str()).unwrap_or("");
    let (time, level, module, message) = (get("time"), get("level"), get("module"), get("message"));
    if message.is_empty() {
        l.to_string()
    } else {
        format!("[{time}] [{level}] [{module}] {message}")
    }
}

fn table_head(is_en: bool) -> String {
    if is_en {
        "<thead><tr><th>Time (s)</th><th>Bandwidth (Mbps)</th><th>Transfer (MB)</th><th>Jitter (ms)</th><th>Loss</th></tr></thead>".to_string()
    } else {
        "<thead><tr><th>时间(s)</th><th>带宽(Mbps)</th><th>传输(MB)</th><th>抖动(ms)</th><th>丢包率</th></tr></thead>".to_string()
    }
}

fn table_rows(points: &[MetricPoint]) -> String {
    points
        .iter()
        .map(|p| {
            format!(
                "<tr><td>{}</td><td>{:.2}</td><td>{:.2}</td><td>{:.2}</td><td>{:.2}%</td></tr>",
                p.second, p.bandwidth_mbps, p.transfer_mb, p.jitter_ms, p.loss_percent
            )
        })
        .collect::<String>()
}

pub(crate) fn status_word(status: &str, is_en: bool) -> String {
    let (done, failed, stopped) = if is_en {
        ("Done", "Failed", "Stopped")
    } else {
        ("已完成", "失败", "已停止")
    };
    match status {
        "success" => done.to_string(),
        "failed" => failed.to_string(),
        "stopped" => stopped.to_string(),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// 内联 SVG 带宽 / 抖动曲线（自包含，报告可离线打开）
// ---------------------------------------------------------------------------

pub(crate) fn svg_curve(points: &[MetricPoint], is_en: bool) -> String {
    let (w, h) = (940.0, 220.0);
    let (pad_l, pad_r, pad_t, pad_b) = (58.0, 18.0, 14.0, 30.0);
    let (plot_w, plot_h) = (w - pad_l - pad_r, h - pad_t - pad_b);
    if points.is_empty() {
        let msg = if is_en { "No data" } else { "无数据" };
        return format!(
            r##"<svg viewBox="0 0 {w} {h}" xmlns="http://www.w3.org/2000/svg" style="width:100%;height:auto;background:#f6f8fb;border-radius:6px"><text x="{w}" y="{h}" text-anchor="end" font-size="14" fill="#98a2b3">{msg}</text></svg>"##
        );
    }
    let use_jitter = points.iter().all(|p| p.bandwidth_mbps <= 0.0);
    let value = |p: &MetricPoint| {
        if use_jitter {
            p.jitter_ms
        } else {
            p.bandwidth_mbps
        }
    };
    let t_min = points.first().map(|p| p.second).unwrap_or(0);
    let t_max = points.last().map(|p| p.second).unwrap_or(0);
    let (mut v_min, mut v_max) = points
        .iter()
        .map(&value)
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
            (lo.min(v), hi.max(v))
        });
    if !v_min.is_finite() || !v_max.is_finite() {
        v_min = 0.0;
        v_max = 1.0
    }
    if v_min == v_max {
        v_min -= 1.0;
        v_max += 1.0
    }
    let x_of = |t: i64| {
        if t_max == t_min {
            pad_l + plot_w / 2.0
        } else {
            pad_l + (t - t_min) as f64 / (t_max - t_min) as f64 * plot_w
        }
    };
    let y_of = |v: f64| pad_t + (1.0 - (v - v_min) / (v_max - v_min)) * plot_h;
    let mut grid = String::new();
    for i in 0..=4 {
        let v = v_min + (v_max - v_min) * i as f64 / 4.0;
        let gy = y_of(v);
        grid.push_str(&format!(
            r##"<line x1="{pad_l}" y1="{gy:.1}" x2="{w}" y2="{gy:.1}" stroke="#e5e9ef"/><text x="{}" y="{:.1}" text-anchor="end" font-size="11" fill="#98a2b3">{v:.1}</text>"##,
            pad_l - 6.0, gy + 4.0
        ));
    }
    let poly = points
        .iter()
        .map(|p| format!("{:.1},{:.1}", x_of(p.second), y_of(value(p))))
        .collect::<Vec<_>>()
        .join(" ");
    let single_point = if points.len() == 1 {
        let p = &points[0];
        format!(
            r##"<circle cx="{:.1}" cy="{:.1}" r="3" fill="#096edc"/>"##,
            x_of(p.second),
            y_of(value(p))
        )
    } else {
        String::new()
    };
    let unit = if use_jitter { "ms" } else { "Mbps" };
    let y_axis = y_of(v_min) + 0.5;
    format!(
        r##"<svg viewBox="0 0 {w} {h}" xmlns="http://www.w3.org/2000/svg" style="width:100%;height:auto;background:#f6f8fb;border-radius:6px">{grid}<line x1="{pad_l}" y1="{y_axis:.1}" x2="{w}" y2="{y_axis:.1}" stroke="#d3dae3"/><text x="{pad_l}" y="{:.1}" font-size="11" fill="#98a2b3">{t_min}s</text><text x="{w}" y="{:.1}" text-anchor="end" font-size="11" fill="#98a2b3">{t_max}s</text><text x="{:.1}" y="{:.1}" text-anchor="end" font-size="11" fill="#98a2b3">{unit}</text><polyline fill="none" stroke="#096edc" stroke-width="2" points="{poly}"/>{single_point}</svg>"##,
        h - 8.0,
        h - 8.0,
        w - 2.0,
        pad_t - 4.0
    )
}

#[cfg(test)]
mod tests {
    use super::{escape_html, render_html, svg_curve};
    use crate::models::{MetricPoint, ReportItem, ReportRequest};
    use serde_json::json;

    #[test]
    fn escape_html_encodes_special_chars() {
        assert_eq!(escape_html("<a>"), "&lt;a&gt;");
        assert_eq!(escape_html("A & B"), "A &amp; B");
        assert_eq!(escape_html(r#""quote""#), "&quot;quote&quot;");
    }

    #[test]
    fn single_sample_curve_renders_a_visible_point() {
        let svg = svg_curve(
            &[MetricPoint {
                second: 1,
                bandwidth_mbps: 100.0,
                ..Default::default()
            }],
            false,
        );
        assert!(svg.contains("<circle"));
        assert!(svg.contains("100.0"));
    }

    #[test]
    fn html_report_contains_print_layout_and_chart() {
        let request = ReportRequest {
            format: "pdf".into(),
            save_path: None,
            locale: "en".into(),
            config: json!({}),
            summary: json!({}),
            points: vec![],
            items: vec![ReportItem {
                label: "TCP Bandwidth".into(),
                status: "success".into(),
                points: vec![MetricPoint {
                    second: 1,
                    bandwidth_mbps: 100.0,
                    ..Default::default()
                }],
            }],
            logs: vec![],
        };
        let html = render_html(&request);
        assert!(html.contains("@page{size:A4"));
        assert!(html.contains("<svg"));
        assert!(html.contains("<circle"));
    }
}
