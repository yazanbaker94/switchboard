fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "set_layout",
            "reload_account",
            "set_overlay",
            "get_diagnostics",
            "set_labels",
            "enable_notifications",
            "notification_permissions",
            "set_compact_sidebar",
            "configure_shortcuts",
        ]),
    ))
    .expect("failed to build Switchboard resources");
}
