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
    // Common-Controls v6 SxS 清单统一走链接器嵌入：tauri::test mock 会链接 muda 的
    // comctl32 v6 专属导入（TaskDialogIndirect 等），无清单时加载器绑定 comctl32 v5，
    // 缺导出符号，测试进程启动即 0xC0000139。tauri-build 默认把 v6 清单作为资源只嵌
    // 入 app 二进制（rustc-link-arg-bins），测试目标拿不到，故改用全目标
    // /MANIFEST:EMBED + /MANIFESTINPUT，并关闭 tauri-build 的资源清单以免
    // RT_MANIFEST 重复（CVT1100）。
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let manifest =
            std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("common-controls-v6.xml");
        std::fs::write(
            &manifest,
            r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
</assembly>
"#,
        )
        .unwrap();
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    }
    let mut attrs = tauri_build::Attributes::new();
    attrs = attrs.windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
    tauri_build::try_build(attrs).expect("tauri_build::try_build 失败")
}
