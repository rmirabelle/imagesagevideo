mod clip_size;
mod crypto;
mod document;
mod engine;
mod files;
mod queue;
mod settings;
mod tray;
mod updater;
mod webm;

use std::{path::Path, sync::Mutex};
use tauri::{Emitter, Manager, State, WindowEvent};

struct PendingOpenDocument(Mutex<Option<String>>);

/// The first argument that names an Image Sage document.
fn document_from_args(args: &[String], cwd: &str) -> Option<String> {
    args.iter().skip(1).find_map(|argument| {
        let path = Path::new(argument);
        if !path.extension()?.to_str()?.eq_ignore_ascii_case("imagesage") {
            return None;
        }
        let resolved = if path.is_absolute() { path.to_path_buf() } else { Path::new(cwd).join(path) };
        Some(resolved.to_string_lossy().into_owned())
    })
}

#[tauri::command]
fn take_pending_open_document(state: State<'_, PendingOpenDocument>) -> Option<String> {
    state.0.lock().unwrap_or_else(|error| error.into_inner()).take()
}

/// Hides the window; the tray icon stays, and the job goes on.
#[tauri::command]
fn hide_to_tray(app: tauri::AppHandle) {
    tray::hide_window(&app);
}

/// Ends the app. A job that runs is kept in the job list and can go on at the next start.
#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    app.exit(0);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let window_state_flags = tauri_plugin_window_state::StateFlags::SIZE
        | tauri_plugin_window_state::StateFlags::POSITION
        | tauri_plugin_window_state::StateFlags::MAXIMIZED;

    let initial_args = std::env::args_os().map(|argument| argument.to_string_lossy().into_owned()).collect::<Vec<_>>();
    let initial_cwd = std::env::current_dir().unwrap_or_default().to_string_lossy().into_owned();
    let initial_document = document_from_args(&initial_args, &initial_cwd);

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
            if let Some(path) = document_from_args(&args, &cwd) {
                let _ = app.emit("open-document-requested", path);
            }
            tray::show_window(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_window_state::Builder::default().with_state_flags(window_state_flags).build())
        .manage(PendingOpenDocument(Mutex::new(initial_document)))
        .manage(engine::Engine::default())
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            let app = window.app_handle();
            match event {
                /* While a job runs, the window asks first: hide to the tray, or quit. */
                WindowEvent::CloseRequested { api, .. } => {
                    if app.state::<queue::Queue>().is_running() {
                        api.prevent_close();
                        let _ = app.emit("close-requested", ());
                    } else {
                        app.exit(0);
                    }
                }
                /* Windows has no event for "minimized"; the window is resized when it minimizes. A minimized window hides to the tray. */
                WindowEvent::Resized(_) => {
                    if window.is_minimized().unwrap_or(false) {
                        let _ = window.hide();
                    }
                }
                _ => {}
            }
        })
        .setup(|app| {
            let handle = app.handle().clone();
            app.manage(queue::restore(&handle));
            tray::build(&handle)?;
            /* A job that waits from the last run shows on the tray icon from the start. */
            queue::show_in_tray(&handle);
            tray::show_window(&handle);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            files::reveal_path,
            files::play_video,
            files::log_frontend_error,
            files::cursor_position,
            document::open_document,
            engine::engine_status,
            engine::engine_download,
            engine::find_models,
            settings::settings_get,
            settings::settings_set,
            settings::settings_from_image_sage,
            queue::queue_get,
            queue::queue_start,
            queue::queue_pause,
            queue::queue_resume,
            queue::queue_clear,
            take_pending_open_document,
            hide_to_tray,
            quit_app,
            updater::check_for_update,
            updater::download_and_run_installer,
            updater::get_app_version
        ])
        .run(tauri::generate_context!())
        .expect("error while running Image Sage Video");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_document_argument_case_insensitively() {
        let args = vec!["imagesagevideo.exe".into(), "Poster.IMAGESAGE".into()];
        assert_eq!(document_from_args(&args, r"C:\Images"), Some(r"C:\Images\Poster.IMAGESAGE".into()));
        let args = vec!["imagesagevideo.exe".into(), r"D:\photo.png".into()];
        assert_eq!(document_from_args(&args, r"C:\Images"), None);
    }
}
