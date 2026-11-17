fn main() {
    // SQLx's stable macro tracks included files, but not newly added migrations.
    println!("cargo:rerun-if-changed=migrations");
    tauri_build::build()
}
