fn main() {
    // 构建时注入 Git commit id：发布包内没有 .git，编译时读取并写入 env，
    // 运行时可经 get_app_info 命令读到（「关于」页展示用）。无 git 环境时回退为 unknown。
    let commit = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default())
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=LINKGAUGE_COMMIT={commit}");
    // commit 变化（提交 / 切换分支）时触发重新编译
    println!("cargo:rerun-if-changed=../.git/HEAD");
    tauri_build::build()
}
