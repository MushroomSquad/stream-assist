fn main() {
    println!("cargo:rerun-if-changed=assets");
    tauri_build::build()
}
