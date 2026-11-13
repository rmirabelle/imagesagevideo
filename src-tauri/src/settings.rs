use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

use crate::{
    crypto,
    engine::{self, ModelFiles},
};

/**
 * The app's settings: Wan 2.2's model files, and the choices of the last
 * job. They are kept in one encrypted file in the app's settings folder.
 * On first use, the model files come from Image Sage's settings, when Image
 * Sage has them.
 */
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub files: ModelFiles,
    /// The folder the last job saved its videos in.
    pub output_dir: String,
    /// How many 5-second clips each layer's video had in the last job.
    pub clips_per_layer: usize,
    /// The folder the last LoRA file was chosen from; the file browser opens there.
    pub lora_folder: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            files: ModelFiles::default(),
            output_dir: String::new(),
            clips_per_layer: crate::queue::DEFAULT_CLIPS_PER_LAYER,
            lora_folder: String::new(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    settings: Settings,
    /// The model files that are not set or not found, by name.
    missing: Vec<&'static str>,
    /// Image Sage has Wan 2.2 settings that can be copied.
    image_sage_available: bool,
    default_negative_prompt: &'static str,
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(|error| format!("Could not find the settings folder: {error}"))?;
    std::fs::create_dir_all(&dir).map_err(|error| format!("Could not create the settings folder: {error}"))?;
    Ok(dir.join("settings.bin"))
}

/// The saved settings. With no saved file (or one that cannot be read), the defaults, with Image Sage's model files when it has them.
pub fn load(app: &AppHandle) -> Settings {
    let saved = settings_path(app)
        .and_then(|path| crypto::read_sealed(&path))
        .ok()
        .flatten()
        .and_then(|bytes| serde_json::from_slice::<Settings>(&bytes).ok());
    saved.unwrap_or_else(|| Settings { files: engine::image_sage_files(app).unwrap_or_default(), ..Settings::default() })
}

pub fn save(app: &AppHandle, settings: &Settings) -> Result<(), String> {
    let bytes = serde_json::to_vec(settings).map_err(|error| format!("Could not save the settings: {error}"))?;
    crypto::write_sealed(&settings_path(app)?, &bytes)
}

#[tauri::command]
pub fn settings_get(app: AppHandle) -> SettingsView {
    let settings = load(&app);
    SettingsView {
        missing: settings.files.missing(),
        settings,
        image_sage_available: engine::image_sage_files(&app).is_some(),
        default_negative_prompt: engine::DEFAULT_NEGATIVE_PROMPT,
    }
}

#[tauri::command]
pub fn settings_set(app: AppHandle, settings: Settings) -> Result<(), String> {
    save(&app, &settings)
}

/// Image Sage's Wan 2.2 image-to-video files, for the settings dialog to show; nothing is saved.
#[tauri::command]
pub fn settings_from_image_sage(app: AppHandle) -> Result<ModelFiles, String> {
    engine::image_sage_files(&app).ok_or_else(|| "Image Sage has no Wan 2.2 image-to-video settings on this computer.".to_string())
}
