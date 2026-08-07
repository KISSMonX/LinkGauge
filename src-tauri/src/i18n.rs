//! 运行时界面语言工具：tr / tr_format! 与语言状态读写。
//!
//! 从 runner.rs 拆分出的独立模块，使 runner、ssh、report_html 等模块
//! 都能直接导入国际化函数，而无需依赖 runner 的测试编排语义。

use std::sync::{Arc, RwLock};

/// 按界面语言选择消息文案（locale 为空时默认中文）
pub(crate) fn tr<'a>(locale: &str, zh: &'a str, en: &'a str) -> &'a str {
    if locale == "en" {
        en
    } else {
        zh
    }
}

/// 按界面语言选择格式化模板（模板必须是字面量，format! 才能编译期检查）
#[macro_export]
macro_rules! tr_format {
    ($locale:expr, $zh:literal, $en:literal $(, $arg:expr)* $(,)?) => {{
        if $locale == "en" {
            format!($en $(, $arg)*)
        } else {
            format!($zh $(, $arg)*)
        }
    }};
}

/// 读取当前界面语言（日志输出时调用，避免使用会话启动时的快照）
pub(crate) fn current_locale(handle: &Arc<RwLock<String>>) -> String {
    handle.read().map(|v| v.clone()).unwrap_or_default()
}
