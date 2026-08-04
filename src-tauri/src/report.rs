use crate::models::ReportRequest;
use chrono::Local;
use std::path::PathBuf;
use std::process::Command;
use tauri::{AppHandle, Manager};
use tokio::fs;

#[tauri::command]
pub async fn generate_report(app: AppHandle, request: ReportRequest) -> Result<String, String> {
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
    let requested = request
        .save_path
        .as_deref()
        .map(PathBuf::from)
        .or_else(|| {
            match request.format.to_lowercase().as_str() {
                "html" => Some(dir.join(format!("linkgauge-report-{stamp}.html"))),
                "pdf" => Some(dir.join(format!("linkgauge-report-{stamp}.pdf"))),
                _ => None,
            }
        });
    let Some(path) = requested else {
        return Err("报告格式仅支持 HTML 或 PDF".into());
    };
    match request.format.to_lowercase().as_str() {
        "html" => write_html(path, &request).await,
        "pdf" => write_pdf(path, &request).await,
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
    let config = serde_json::to_string_pretty(&request.config).unwrap_or_default();
    let summary = serde_json::to_string_pretty(&request.summary).unwrap_or_default();
    let rows = request
        .points
        .iter()
        .map(|p| {
            format!(
                "<tr><td>{}</td><td>{:.2}</td><td>{:.2}</td><td>{:.2}</td><td>{:.2}%</td></tr>",
                p.second, p.bandwidth_mbps, p.transfer_mb, p.jitter_ms, p.loss_percent
            )
        })
        .collect::<String>();
    let logs = request
        .logs
        .iter()
        .map(|l| format!("<div>{}</div>", escape_html(&l.to_string())))
        .collect::<String>();
    // 报告语言跟随界面语言（默认中文）
    let is_en = request.locale == "en";
    let html = if is_en {
        format!(
            r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><title>LinkGauge Test Report</title><style>body{{font:14px Arial,'Microsoft YaHei';max-width:1000px;margin:35px auto;color:#172033}}h1{{color:#096edc}}section{{border:1px solid #dce2ea;border-radius:8px;padding:18px;margin:16px 0}}pre{{background:#f6f8fb;padding:14px;white-space:pre-wrap}}table{{width:100%;border-collapse:collapse}}th,td{{padding:9px;border-bottom:1px solid #e5e9ef;text-align:right}}th:first-child,td:first-child{{text-align:left}}.logs{{font:12px Consolas;max-height:360px;overflow:auto}}</style></head><body><h1>LinkGauge Test Report</h1><p>Generated: {}</p><section><h2>Test Configuration</h2><pre>{}</pre></section><section><h2>Statistics</h2><pre>{}</pre></section><section><h2>Test Data</h2><table><thead><tr><th>Time (s)</th><th>Bandwidth (Mbps)</th><th>Transfer (MB)</th><th>Jitter (ms)</th><th>Loss</th></tr></thead><tbody>{}</tbody></table></section><section><h2>Run Logs</h2><div class="logs">{}</div></section></body></html>"#,
            Local::now().format("%Y-%m-%d %H:%M:%S"),
            escape_html(&config),
            escape_html(&summary),
            rows,
            logs
        )
    } else {
        format!(
            r#"<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><title>LinkGauge 测试报告</title><style>body{{font:14px Arial,'Microsoft YaHei';max-width:1000px;margin:35px auto;color:#172033}}h1{{color:#096edc}}section{{border:1px solid #dce2ea;border-radius:8px;padding:18px;margin:16px 0}}pre{{background:#f6f8fb;padding:14px;white-space:pre-wrap}}table{{width:100%;border-collapse:collapse}}th,td{{padding:9px;border-bottom:1px solid #e5e9ef;text-align:right}}th:first-child,td:first-child{{text-align:left}}.logs{{font:12px Consolas;max-height:360px;overflow:auto}}</style></head><body><h1>LinkGauge 测试报告</h1><p>生成时间：{}</p><section><h2>测试配置</h2><pre>{}</pre></section><section><h2>统计结果</h2><pre>{}</pre></section><section><h2>测试数据</h2><table><thead><tr><th>时间(s)</th><th>带宽(Mbps)</th><th>传输(MB)</th><th>抖动(ms)</th><th>丢包率</th></tr></thead><tbody>{}</tbody></table></section><section><h2>执行日志</h2><div class="logs">{}</div></section></body></html>"#,
            Local::now().format("%Y-%m-%d %H:%M:%S"),
            escape_html(&config),
            escape_html(&summary),
            rows,
            logs
        )
    };
    fs::write(&path, html)
        .await
        .map_err(|e| format!("写入 HTML 报告失败：{e}"))?;
    Ok(path.to_string_lossy().to_string())
}

async fn write_pdf(path: PathBuf, request: &ReportRequest) -> Result<String, String> {
    let avg = request
        .summary
        .get("averageBandwidth")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let max = request
        .summary
        .get("maxBandwidth")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let loss = request
        .summary
        .get("lossPercent")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let lines = vec![
        "LinkGauge Test Report".to_string(),
        format!("Generated: {}", Local::now().format("%Y-%m-%d %H:%M:%S")),
        format!("Average bandwidth: {avg:.2} Mbps"),
        format!("Maximum bandwidth: {max:.2} Mbps"),
        format!("Packet loss: {loss:.2}%"),
        format!("Samples: {}", request.points.len()),
        format!("Log entries: {}", request.logs.len()),
    ];
    let content = lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            format!(
                "BT /F1 {} Tf 55 {} Td ({}) Tj ET\n",
                if i == 0 { 20 } else { 12 },
                780 - (i as i32 * 32),
                pdf_escape(l)
            )
        })
        .collect::<String>();
    let mut objects = vec![String::new(), "<< /Type /Catalog /Pages 2 0 R >>".into(), "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(), "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".into(), format!("<< /Length {} >>\nstream\n{}endstream", content.len(), content), "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into()];
    let mut pdf = "%PDF-1.4\n".to_string();
    let mut offsets = vec![0usize];
    for (i, obj) in objects.iter_mut().enumerate().skip(1) {
        offsets.push(pdf.len());
        pdf.push_str(&format!("{} 0 obj\n{}\nendobj\n", i, obj));
    }
    let xref = pdf.len();
    pdf.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", objects.len()));
    for offset in offsets.iter().skip(1) {
        pdf.push_str(&format!("{:010} 00000 n \n", offset));
    }
    pdf.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF",
        objects.len(),
        xref
    ));
    fs::write(&path, pdf.as_bytes())
        .await
        .map_err(|e| format!("写入 PDF 报告失败：{e}"))?;
    Ok(path.to_string_lossy().to_string())
}
fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn pdf_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('(', "\\(")
        .replace(')', "\\)")
}
