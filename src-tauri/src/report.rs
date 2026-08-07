//! 测试报告生成：HTML 写入文件 或 PDF 打印。
//!
//! HTML 渲染委托给 crate::report_html，本模块只负责文件 I/O、
//! 目录管理、PDF WebView 打印窗口。
//!
//! 不引入 Chrome/wkhtmltopdf 等外部依赖——PDF 复用 Tauri WebView
//! 自带打印引擎完成分页与输出。

use crate::models::ReportRequest;
use crate::report_html;
use chrono::Local;
use std::path::PathBuf;
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

/// 用系统文件管理器打开报告输出目录
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
    crate::system::open_path_in_shell(&dir)?;
    Ok(dir.to_string_lossy().to_string())
}

async fn write_html(path: PathBuf, request: &ReportRequest) -> Result<String, String> {
    let html = report_html::render_html(request);
    fs::write(&path, html)
        .await
        .map_err(|e| format!("写入 HTML 报告失败：{e}"))?;
    Ok(path.to_string_lossy().to_string())
}

/// PDF 直接复用 HTML 渲染结果，由系统 WebView 的打印引擎完成分页与 PDF 输出
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
    let html = report_html::render_html(request).replace(
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
