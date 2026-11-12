use std::path::Path;

/// A path as Windows writes it. File Explorer does not accept forward slashes.
pub fn windows_path(path: &str) -> String {
    if cfg!(windows) { path.trim().replace('/', "\\") } else { path.trim().to_string() }
}

/// Runs File Explorer with one raw argument, so a path with spaces stays one path.
fn run_explorer(argument: String) -> Result<(), String> {
    let mut command = std::process::Command::new("explorer");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.raw_arg(argument);
    }
    #[cfg(not(windows))]
    command.arg(argument);
    command.spawn().map(|_| ()).map_err(|error| format!("Could not open File Explorer: {error}"))
}

/// Opens a folder in File Explorer, or the folder of a file with the file selected.
#[tauri::command]
pub fn reveal_path(path: String) -> Result<(), String> {
    let path = windows_path(&path);
    let target = Path::new(&path);
    if target.is_dir() {
        run_explorer(format!("\"{path}\""))
    } else if target.is_file() {
        run_explorer(format!("/select,\"{path}\""))
    } else {
        Err(format!("{path} no longer exists"))
    }
}

/**
 * Plays a finished video in the default video app. Only existing .webm
 * files are opened, so the webview cannot use this to start a program.
 */
#[tauri::command]
pub fn play_video(path: String) -> Result<(), String> {
    let path = windows_path(&path);
    let file = Path::new(&path);
    let is_webm = file.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("webm"));
    if !is_webm || !file.is_file() {
        return Err(format!("{path} is not a WebM video that exists"));
    }
    run_explorer(format!("\"{path}\""))
}

/// Writes an error from the webview to the app's error output, so crashes show in the dev log.
#[tauri::command]
pub fn log_frontend_error(message: String) {
    eprintln!("[imagesagevideo] frontend error: {}", message.chars().take(4000).collect::<String>());
}

/// Returns the OS cursor position in physical pixels, for the window-control hover fix.
#[tauri::command]
pub fn cursor_position() -> (i32, i32) {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
        let mut point = POINT { x: 0, y: 0 };
        unsafe {
            let _ = GetCursorPos(&mut point);
        }
        (point.x, point.y)
    }
    #[cfg(not(windows))]
    {
        (0, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn paths_use_backslashes() {
        assert_eq!(windows_path(" D:/Videos/out "), r"D:\Videos\out");
        assert_eq!(windows_path(r"D:\Videos\out"), r"D:\Videos\out");
    }
}
