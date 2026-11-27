// Hide the console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod launch;

/// Opens the project page in the default browser. Only the project's own
/// GitHub links are allowed, so the window cannot start arbitrary addresses.
#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    if !url.starts_with("https://github.com/Sp1KLe/") {
        return Err("address is not allowed".into());
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // 0x08000000 = CREATE_NO_WINDOW, so no console flashes up.
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url.as_str()])
            .creation_flags(0x0800_0000)
            .spawn()
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

fn main() {
    tauri::Builder::default()
        .manage(launch::Busy::default())
        .invoke_handler(tauri::generate_handler![
            open_url,
            launch::paths,
            launch::env_info,
            launch::play,
            launch::reset_all
        ])
        .run(tauri::generate_context!())
        .expect("error while running Bearite-PC");
}
