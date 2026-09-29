fn main() {
    println!("cargo:rerun-if-env-changed=AIRSEND_BUILD_REVISION");
    tauri_build::build()
}
