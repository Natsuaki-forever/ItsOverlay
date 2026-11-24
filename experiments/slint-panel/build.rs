fn main() {
    println!("cargo:rerun-if-changed=../../assets/app-icon.rc");
    println!("cargo:rerun-if-changed=../../assets/app-icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let rc_file = std::path::Path::new("../../assets/app-icon.rc");
        if rc_file.exists() {
            if let Ok(out_dir) = std::env::var("OUT_DIR") {
                let res_path = std::path::Path::new(&out_dir).join("app-icon.res");
                if let Ok(status) = std::process::Command::new("rc.exe")
                    .arg("/nologo")
                    .arg("/fo")
                    .arg(&res_path)
                    .arg(rc_file)
                    .status()
                {
                    if status.success() {
                        println!("cargo:rustc-link-arg-bins={}", res_path.display());
                    }
                }
            }
        }
    }
    slint_build::compile("panel.slint").expect("could not compile editor layout");
}
