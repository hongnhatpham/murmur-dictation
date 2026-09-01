fn main() {
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR missing"));
    let icon = output.join("murmur.ico");
    let bytes = minimal_icon();
    std::fs::write(&icon, &bytes).expect("failed to create build icon");
    let source_icon = std::path::PathBuf::from("icons/icon.ico");
    if let Some(parent) = source_icon.parent() {
        std::fs::create_dir_all(parent).expect("failed to create icon directory");
    }
    std::fs::write(source_icon, bytes).expect("failed to create source icon");
    let windows = tauri_build::WindowsAttributes::new().window_icon_path(icon);
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows))
        .expect("failed to run Tauri build script");
}

/// A 1x1 orange ICO keeps development builds self-contained until product artwork lands.
fn minimal_icon() -> Vec<u8> {
    let mut bytes = vec![
        0, 0, 1, 0, 1, 0, // ICO header
        1, 1, 0, 0, 1, 0, 32, 0, 48, 0, 0, 0, 22, 0, 0, 0, // directory entry
        40, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 1, 0, 32, 0, // bitmap header
        0, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    bytes.extend_from_slice(&[0x32, 0x7a, 0xff, 0xff]); // BGRA pixel
    bytes.extend_from_slice(&[0, 0, 0, 0]); // AND mask row
    bytes
}
