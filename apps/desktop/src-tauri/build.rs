fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "select_workspace_directory",
            "desktop_status",
            "set_launch_at_login",
            "check_for_update",
            "install_update",
        ]),
    ))
    .expect("Tauri build configuration");
}
