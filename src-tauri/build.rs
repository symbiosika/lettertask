fn main() {
    // Phone build mode (see MOBILE_MODE in lib.rs); printed so the build log
    // shows what was compiled in.
    println!("cargo:rerun-if-changed=mobile-mode.txt");
    let mode = std::fs::read_to_string("mobile-mode.txt").unwrap_or_default();
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("ios") || target.contains("android") {
        println!("cargo:warning=lettertask Handy-Modus: {} ({target})", mode.trim());
    }
    tauri_build::build()
}
