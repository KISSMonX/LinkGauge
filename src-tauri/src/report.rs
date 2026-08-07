use crate::models::{MetricPoint, ReportRequest};
use chrono::Local;
use std::path::PathBuf;
use std::process::Command;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use tokio::fs;

#[tauri::command]
pub async fn generate_report(app: AppHandle, request: ReportRequest) -> Result<String, String> {
    if request.format.eq_ignore_ascii_case("pdf") {
        return print_html(app, &request).await;
    }
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("reports");
    fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("无法创建报告目录：{e}"))?;
    // 用户通过保存对话框指定了完整路径时写入该路径，否则使用默认目录 + 时间戳文件名
    let stamp = Local::now().format("%Y%m%d%H%M%S");
    let requested = request.save_path.as_deref().map(PathBuf::from).or_else(|| {
        match request.format.to_lowercase().as_str() {
            "html" => Some(dir.join(format!("linkgauge-report-{stamp}.html"))),
            _ => None,
        }
    });
    let Some(path) = requested else {
        return Err("报告格式仅支持 HTML 或 PDF".into());
    };
    match request.format.to_lowercase().as_str() {
        "html" => write_html(path, &request).await,
        _ => Err("报告格式仅支持 HTML 或 PDF".into()),
    }
}

/// 返回默认报告输出目录路径（不存在则创建），作为保存对话框的默认位置
#[tauri::command]
pub async fn get_report_dir(app: AppHandle) -> Result<String, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("reports");
    fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("无法创建报告目录：{e}"))?;
    Ok(dir.to_string_lossy().to_string())
}

/// 用系统文件管理器打开报告输出目录，返回目录路径
#[tauri::command]
pub async fn open_report_dir(app: AppHandle) -> Result<String, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("reports");
    fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("无法创建报告目录：{e}"))?;
    let result = Command::new(if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    })
    .arg(&dir)
    .spawn()
    .map_err(|e| format!("无法打开报告目录：{e}"))?;
    drop(result);
    Ok(dir.to_string_lossy().to_string())
}

async fn write_html(path: PathBuf, request: &ReportRequest) -> Result<String, String> {
    let html = render_html(request);
    fs::write(&path, html)
        .await
        .map_err(|e| format!("写入 HTML 报告失败：{e}"))?;
    Ok(path.to_string_lossy().to_string())
}

/// PDF 直接复用 HTML 渲染结果，由系统 WebView 的打印引擎完成分页与 PDF 输出。
/// 不调用 Chrome/wkhtmltopdf 等外部程序，保持安装包不依赖外部可执行文件。
async fn print_html(app: AppHandle, request: &ReportRequest) -> Result<String, String> {
    let is_en = request.locale == "en";
    let dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?
        .join("print");
    fs::create_dir_all(&dir).await.map_err(|e| {
        if is_en {
            format!("Failed to create the print cache directory: {e}")
        } else {
            format!("无法创建打印缓存目录：{e}")
        }
    })?;
    let path = dir.join("linkgauge-report-print.html");
    let html = render_html(request).replace(
        "</body>",
        r#"<script>addEventListener('load',()=>setTimeout(()=>window.print(),250));</script></body>"#,
    );
    fs::write(&path, html).await.map_err(|e| {
        if is_en {
            format!("Failed to prepare the PDF print page: {e}")
        } else {
            format!("无法准备 PDF 打印页面：{e}")
        }
    })?;
    let url = tauri::Url::from_file_path(&path).map_err(|_| {
        if is_en {
            format!("Failed to open the print page: {}", path.to_string_lossy())
        } else {
            format!("无法打开打印页面：{}", path.to_string_lossy())
        }
    })?;
    if let Some(window) = app.get_webview_window("report-print") {
        window.destroy().map_err(|e| e.to_string())?;
    }
    WebviewWindowBuilder::new(&app, "report-print", WebviewUrl::External(url))
        .title(if request.locale == "en" {
            "LinkGauge Report · Print / Save as PDF"
        } else {
            "LinkGauge 报告 · 打印 / 保存为 PDF"
        })
        .inner_size(1100.0, 820.0)
        .min_inner_size(800.0, 600.0)
        .center()
        .build()
        .map_err(|e| {
            if is_en {
                format!("Failed to open the PDF print window: {e}")
            } else {
                format!("无法打开 PDF 打印窗口：{e}")
            }
        })?;
    Ok(path.to_string_lossy().to_string())
}

fn render_html(request: &ReportRequest) -> String {
    let logs = request
        .logs
        .iter()
        .map(|l| format!("<div>{}</div>", escape_html(&log_line(l))))
        .collect::<String>();
    // 报告语言跟随界面语言（默认中文）
    let is_en = request.locale == "en";
    // 配置与统计以两列表格呈现（JSON 不适合作为报告内容），数值/枚举转可读文本
    let config_section = config_section_html(request, is_en);
    let stats_section = stats_section_html(request, is_en);
    // 数据区：按测试项目分组，每项一个 section（标题 + 曲线 + 数据表）；
    // 无分组数据时回退为单一数据表（兼容旧版调用）
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
            .map(|item| {
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
            })
            .collect::<String>()
    };
    let html = if is_en {
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
    };
    html
}
fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// 测试配置参数的显示顺序与本地化标签（(key, 中文, English)）
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

/// 统计结果的显示顺序与本地化标签
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

/// 测试配置 section：两列表格（参数 / 值）
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

/// 统计结果 section：两列表格（统计项 / 值）
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

/// 通用两列表格：按 ORDER 顺序输出，键转本地化标签、值转可读文本；缺失的键跳过
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

/// 单元格 HTML：普通值转可读文本并转义；数组（如日志文件列表）逐项转义后换行展示
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

/// 值可读化：mode 枚举转本地化文字；数字整数值不带小数、其余保留两位
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

/// 日志条目（JSON）→ 可读文本行：[时间] [级别] [模块] 消息
fn log_line(l: &serde_json::Value) -> String {
    let get = |k: &str| l.get(k).and_then(|v| v.as_str()).unwrap_or("");
    let (time, level, module, message) = (get("time"), get("level"), get("module"), get("message"));
    if message.is_empty() {
        l.to_string()
    } else {
        format!("[{time}] [{level}] [{module}] {message}")
    }
}

/// 数据表列标题（随报告语言）
fn table_head(is_en: bool) -> String {
    if is_en {
        "<thead><tr><th>Time (s)</th><th>Bandwidth (Mbps)</th><th>Transfer (MB)</th><th>Jitter (ms)</th><th>Loss</th></tr></thead>".to_string()
    } else {
        "<thead><tr><th>时间(s)</th><th>带宽(Mbps)</th><th>传输(MB)</th><th>抖动(ms)</th><th>丢包率</th></tr></thead>".to_string()
    }
}

/// 数据表行（与旧版单表一致的 5 列）
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

/// 测试项目结束状态词（随报告语言）
fn status_word(status: &str, is_en: bool) -> String {
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

/// 生成带宽曲线的内联 SVG（自包含、无外部依赖，报告可离线打开）：
/// 横轴时间(s)、纵轴带宽(Mbps)；带宽全为 0 的项目（如 ping）改画抖动(ms)；
/// 无数据时输出占位提示。
fn svg_curve(points: &[MetricPoint], is_en: bool) -> String {
    let (w, h) = (940.0, 220.0);
    let (pad_l, pad_r, pad_t, pad_b) = (58.0, 18.0, 14.0, 30.0);
    let (plot_w, plot_h) = (w - pad_l - pad_r, h - pad_t - pad_b);
    if points.is_empty() {
        let msg = if is_en { "No data" } else { "无数据" };
        return format!(
            r##"<svg viewBox="0 0 {w} {h}" xmlns="http://www.w3.org/2000/svg" style="width:100%;height:auto;background:#f6f8fb;border-radius:6px"><text x="{w}" y="{h}" text-anchor="end" font-size="14" fill="#98a2b3">{msg}</text></svg>"##
        );
    }
    // 带宽全为 0（如 ping 项目）时改画抖动曲线
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
    // 平坦数据（全同值）时上下各留 1 单位，避免除零
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
    // 横向网格线与左侧刻度
    let mut grid = String::new();
    for i in 0..=4 {
        let v = v_min + (v_max - v_min) * i as f64 / 4.0;
        let gy = y_of(v);
        let (gx, gty) = (pad_l - 6.0, gy + 4.0);
        grid.push_str(&format!(
            r##"<line x1="{pad_l}" y1="{gy:.1}" x2="{w}" y2="{gy:.1}" stroke="#e5e9ef"/><text x="{gx}" y="{gty}" text-anchor="end" font-size="11" fill="#98a2b3">{v:.1}</text>"##
        ));
    }
    let poly = points
        .iter()
        .map(|p| format!("{:.1},{:.1}", x_of(p.second), y_of(value(p))))
        .collect::<Vec<_>>()
        .join(" ");
    // 单个样本无法形成线段（按量测试可能短于一个采样周期），至少画出数据点。
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
    // 底部时间刻度与右上单位标签的坐标
    let (t_min_x, t_min_y) = (pad_l, h - 8.0);
    let (t_max_x, t_max_y) = (w, h - 8.0);
    let (unit_x, unit_y) = (w - 2.0, pad_t - 4.0);
    format!(
        r##"<svg viewBox="0 0 {w} {h}" xmlns="http://www.w3.org/2000/svg" style="width:100%;height:auto;background:#f6f8fb;border-radius:6px">{grid}<line x1="{pad_l}" y1="{y_axis:.1}" x2="{w}" y2="{y_axis:.1}" stroke="#d3dae3"/><text x="{t_min_x}" y="{t_min_y}" font-size="11" fill="#98a2b3">{t_min}s</text><text x="{t_max_x}" y="{t_max_y}" text-anchor="end" font-size="11" fill="#98a2b3">{t_max}s</text><text x="{unit_x}" y="{unit_y}" text-anchor="end" font-size="11" fill="#98a2b3">{unit}</text><polyline fill="none" stroke="#096edc" stroke-width="2" points="{poly}"/>{single_point}</svg>"##
    )
}

#[cfg(test)]
mod tests {
    use super::{render_html, svg_curve};
    use crate::models::{MetricPoint, ReportItem, ReportRequest};
    use serde_json::json;

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
            points: Vec::new(),
            items: vec![ReportItem {
                label: "TCP Bandwidth".into(),
                status: "success".into(),
                points: vec![MetricPoint {
                    second: 1,
                    bandwidth_mbps: 100.0,
                    ..Default::default()
                }],
            }],
            logs: Vec::new(),
        };

        let html = render_html(&request);
        assert!(html.contains("@page{size:A4"));
        assert!(html.contains("<svg"));
        assert!(html.contains("<circle"));
    }
}
