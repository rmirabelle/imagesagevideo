use std::path::{Path, PathBuf};

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

/// The files the app may rename or delete: the files it opens and the videos it makes.
const MANAGED_EXTENSIONS: [&str; 6] = ["imagesage", "png", "jpg", "jpeg", "webp", "webm"];

/// True when two paths name one file, however they are written.
pub fn same_file(first: &str, second: &str) -> bool {
    windows_path(first).to_lowercase() == windows_path(second).to_lowercase()
}

/**
 * The path of a file that the app may rename or delete. Only documents,
 * photos and videos that exist are accepted, so the webview cannot use
 * these commands on other files.
 */
pub fn managed_file(path: &str) -> Result<PathBuf, String> {
    let path = windows_path(path);
    let file = Path::new(&path);
    let managed = file
        .extension()
        .is_some_and(|extension| MANAGED_EXTENSIONS.iter().any(|known| extension.eq_ignore_ascii_case(known)));
    if !managed || !file.is_absolute() {
        return Err(format!("{path} is not a document, a photo or a video"));
    }
    if !file.is_file() {
        return Err(format!("{path} no longer exists"));
    }
    Ok(file.to_path_buf())
}

/**
 * The path of `old` with a new name, in the same folder and with the same
 * extension. The user writes the name without the extension; an extension
 * that the user wrote also is not added again.
 */
pub fn renamed_path(old: &Path, name: &str) -> Result<PathBuf, String> {
    let extension = old.extension().map(|extension| extension.to_string_lossy().into_owned()).unwrap_or_default();
    let suffix = format!(".{extension}").to_lowercase();
    let mut name = name.trim();
    if name.to_lowercase().ends_with(&suffix) && name.is_char_boundary(name.len() - suffix.len()) {
        name = name[..name.len() - suffix.len()].trim_end();
    }
    if name.is_empty() {
        return Err("Write a name for the file.".into());
    }
    if name.ends_with('.') || name.chars().any(|character| character.is_control() || r#"\/:*?"<>|"#.contains(character)) {
        return Err(r#"A file name cannot contain \ / : * ? " < > | and cannot end with a dot."#.into());
    }
    Ok(old.with_file_name(format!("{name}.{extension}")))
}

/// Moves a file to the Recycle Bin, so the user can get it back.
pub fn recycle(path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::core::PCWSTR;
        use windows::Win32::UI::Shell::{SHFileOperationW, SHFILEOPSTRUCTW};
        /* FO_DELETE, and FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_SILENT and FOF_NOERRORUI. */
        const DELETE: u32 = 3;
        const TO_RECYCLE_BIN_NO_WINDOWS: u16 = 0x0040 | 0x0010 | 0x0004 | 0x0400;
        /* The list of files ends with two zeros. */
        let from: Vec<u16> = path.as_os_str().encode_wide().chain([0, 0]).collect();
        let mut operation = SHFILEOPSTRUCTW { wFunc: DELETE, pFrom: PCWSTR(from.as_ptr()), fFlags: TO_RECYCLE_BIN_NO_WINDOWS, ..Default::default() };
        let code = unsafe { SHFileOperationW(&mut operation) };
        if code != 0 || operation.fAnyOperationsAborted.as_bool() || path.exists() {
            return Err(format!("Could not move {} to the Recycle Bin (code {code}).", path.display()));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    std::fs::remove_file(path).map_err(|error| format!("Could not delete {}: {error}", path.display()))
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
        assert!(same_file("D:/Videos/Fox.webm", r"d:\videos\fox.webm"));
    }

    #[test]
    fn a_new_name_keeps_the_folder_and_the_extension() {
        let old = Path::new("Photos").join("beach.JPG");
        assert_eq!(renamed_path(&old, " sunset ").unwrap(), Path::new("Photos").join("sunset.JPG"));
        assert_eq!(renamed_path(&old, "sunset.jpg").unwrap(), Path::new("Photos").join("sunset.JPG"));
        assert_eq!(renamed_path(&old, "day.one").unwrap(), Path::new("Photos").join("day.one.JPG"));
        for bad in ["", "  ", ".jpg ", "a/b", r"..\b", "what?", "end."] {
            assert!(renamed_path(&old, bad).is_err(), "{bad}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_deleted_file_goes_to_the_recycle_bin() {
        let file = std::env::temp_dir().join(format!("imagesagevideo-recycle-{}.webm", std::process::id()));
        std::fs::write(&file, b"x").unwrap();
        assert!(managed_file(&file.to_string_lossy()).is_ok());
        recycle(&file).unwrap();
        assert!(!file.exists());
        assert!(managed_file(&file.to_string_lossy()).is_err());
    }
}
