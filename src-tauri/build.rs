fn main() {
    // Icon-only edits must rebuild the executable's Windows resources and
    // Tauri's embedded default window icon, including after `cargo check`.
    println!("cargo:rerun-if-changed=icons");
    tauri_build::build();
}
