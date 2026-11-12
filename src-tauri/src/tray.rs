use std::sync::Mutex;
use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, Wry,
};

use crate::{engine::Engine, queue};

/**
 * The tray icon. It is there from the start of the app to its end. When the
 * window is minimized, the window hides and only the tray icon stays; a
 * click on the icon shows the window again. The icon's tooltip shows the
 * job's progress, and its menu can pause the job and go on with it.
 *
 * A colored dot on the icon shows the job's state: orange while videos are
 * made, green when the job is finished, gray while it is paused. With no
 * job, the icon has no dot.
 */
const TRAY_ID: &str = "main";

/// What the tray icon shows about the job.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TrayState {
    /// There is no job.
    Idle,
    /// Videos are made now.
    Working,
    Paused,
    /// Every clip is done or failed.
    Finished,
}

impl TrayState {
    /// The color of the dot on the icon, as red, green and blue; none for no dot.
    fn dot(self) -> Option<[u8; 3]> {
        match self {
            Self::Idle => None,
            Self::Working => Some([0xff, 0x9f, 0x1a]),
            Self::Paused => Some([0x9a, 0xa0, 0xad]),
            Self::Finished => Some([0x2f, 0xd1, 0x5c]),
        }
    }
}

/// The menu items whose state follows the job, and the state the icon shows now.
pub struct TrayItems {
    pause: MenuItem<Wry>,
    resume: MenuItem<Wry>,
    shown: Mutex<TrayState>,
}

/**
 * Draws a round dot of `color` in the lower right corner of an icon (RGBA
 * pixels, `width` x `height`). The dot has a dark ring, so it shows on a
 * light and on a dark taskbar. Its edge is smoothed.
 */
fn draw_dot(pixels: &mut [u8], width: u32, height: u32, color: [u8; 3]) {
    let side = width.min(height) as f32;
    let radius = side * 0.3;
    let ring = (side * 0.07).max(1.0);
    let (center_x, center_y) = (width as f32 - radius - 0.5, height as f32 - radius - 0.5);
    for y in 0..height {
        for x in 0..width {
            let distance = ((x as f32 + 0.5 - center_x).powi(2) + (y as f32 + 0.5 - center_y).powi(2)).sqrt();
            /* How much of this pixel the dot covers, and how much of that is the colored part inside the ring. */
            let cover = (radius - distance + 0.5).clamp(0.0, 1.0);
            if cover == 0.0 {
                continue;
            }
            let inside = (radius - ring - distance + 0.5).clamp(0.0, 1.0);
            let at = ((y * width + x) * 4) as usize;
            for channel in 0..3 {
                let dot = f32::from(color[channel]) * inside + 24.0 * (1.0 - inside);
                pixels[at + channel] = (dot * cover + f32::from(pixels[at + channel]) * (1.0 - cover)).round() as u8;
            }
            pixels[at + 3] = (255.0 * cover + f32::from(pixels[at + 3]) * (1.0 - cover)).round() as u8;
        }
    }
}

/// The app icon with the state's dot on it.
fn state_icon(app: &AppHandle, state: TrayState) -> Option<Image<'static>> {
    let icon = app.default_window_icon()?;
    let mut pixels = icon.rgba().to_vec();
    if let Some(color) = state.dot() {
        draw_dot(&mut pixels, icon.width(), icon.height(), color);
    }
    Some(Image::new_owned(pixels, icon.width(), icon.height()))
}

pub fn show_window(app: &AppHandle) {
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.show();
        let _ = main.unminimize();
        let _ = main.set_focus();
    }
}

pub fn hide_window(app: &AppHandle) {
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.hide();
    }
}

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show Image Sage Video", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, "pause", "Pause and Unload the Model", false, None::<&str>)?;
    let resume = MenuItem::with_id(app, "resume", "Resume", false, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &pause, &resume, &quit])?;
    let mut tray = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Image Sage Video")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_window(app),
            "pause" => queue::pause(app, &app.state::<queue::Queue>(), &app.state::<Engine>()),
            "resume" => queue::resume(app, &app.state::<queue::Queue>(), &app.state::<Engine>()),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    app.manage(TrayItems { pause, resume, shown: Mutex::new(TrayState::Idle) });
    Ok(())
}

/// Shows the job's state on the icon and its progress in the tooltip, and turns the Pause and Resume menu items on or off.
pub fn update(app: &AppHandle, tooltip: &str, state: TrayState) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return };
    let _ = tray.set_tooltip(Some(tooltip));
    let Some(items) = app.try_state::<TrayItems>() else { return };
    let _ = items.pause.set_enabled(state == TrayState::Working);
    let _ = items.resume.set_enabled(state == TrayState::Paused);
    /* The icon changes only when the state does; the tooltip changes at each sampling step. */
    let mut shown = items.shown.lock().unwrap_or_else(|error| error.into_inner());
    if *shown != state {
        *shown = state;
        let _ = tray.set_icon(state_icon(app, state));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dot_is_in_the_lower_right_corner() {
        let mut pixels = vec![0_u8; 32 * 32 * 4];
        draw_dot(&mut pixels, 32, 32, [0xff, 0x9f, 0x1a]);
        let pixel = |x: usize, y: usize| &pixels[(y * 32 + x) * 4..(y * 32 + x) * 4 + 4];
        assert_eq!(pixel(22, 22), [0xff, 0x9f, 0x1a, 255]);
        assert_eq!(pixel(4, 4), [0, 0, 0, 0]);
        assert_eq!(pixel(4, 28), [0, 0, 0, 0]);
        assert_eq!(pixel(22, 13)[..3], [24, 24, 24]);
    }

    #[test]
    fn each_state_has_its_own_dot() {
        assert_eq!(TrayState::Idle.dot(), None);
        assert_ne!(TrayState::Working.dot(), TrayState::Finished.dot());
        assert_ne!(TrayState::Paused.dot(), TrayState::Finished.dot());
    }
}
