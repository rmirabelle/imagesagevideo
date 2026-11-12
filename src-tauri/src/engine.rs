use base64::{engine::general_purpose::STANDARD, Engine as _};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::AsyncWriteExt;

use crate::clip_size::{CLIP_FRAMES, FPS};

/**
 * The local engine, the same one Image Sage uses: stable-diffusion.cpp's
 * `sd-server.exe` (CUDA 12 build) runs Wan 2.2 image-to-video on this
 * computer. When Image Sage has the engine installed, this app runs that
 * copy; if not, it downloads its own from the same pinned upstream release
 * and checks it against its SHA-256. The model files are the user's own
 * (for example a ComfyUI models folder) and are read in place.
 *
 * The engine keeps the model loaded between clips. `Engine::stop` ends the
 * engine process, which frees the GPU memory; Pause uses it.
 */
const ENGINE_BUILD: &str = "sdcpp-77b72a2";
const ENGINE_URL_PREFIX: &str = "https://github.com/leejet/stable-diffusion.cpp/releases/download/master-";
const ENGINE_MARKER: &str = "installed.json";
const SERVER_EXE: &str = "sd-server.exe";
const IMAGE_SAGE_FOLDER: &str = "com.robmirabelle.imagesage";
const LOAD_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const POLL_INTERVAL: Duration = Duration::from_millis(300);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(3 * 60 * 60);
const LOG_LINES: usize = 20;
/// Sampling steps for one clip, shared by Wan 2.2's two models.
pub const VIDEO_STEPS: u32 = 20;
const CFG_SCALE: f64 = 3.5;

struct EngineZip {
    name: &'static str,
    url: &'static str,
    size: u64,
    sha256: &'static str,
}

const ENGINE_ZIPS: [EngineZip; 2] = [
    EngineZip {
        name: "sd-master-77b72a2-bin-win-cuda12-x64.zip",
        url: "https://github.com/leejet/stable-diffusion.cpp/releases/download/master-1024-77b72a2/sd-master-77b72a2-bin-win-cuda12-x64.zip",
        size: 350_857_718,
        sha256: "e60e538b86ecd47d8e141297fa45734125fdf97925d8ed3b49acb1397b5ee90b",
    },
    EngineZip {
        name: "cudart-sd-bin-win-cu12-x64.zip",
        url: "https://github.com/leejet/stable-diffusion.cpp/releases/download/master-1024-77b72a2/cudart-sd-bin-win-cu12-x64.zip",
        size: 563_452_046,
        sha256: "fe20366827d357c00797eebb58244dddab7fd9a348d70090c3871004c320f38d",
    },
];

/**
 * Wan 2.2's default negative prompt: its makers' list, in English. It works
 * against still pictures, washed-out colors, and bad hands and bodies.
 */
pub const DEFAULT_NEGATIVE_PROMPT: &str = "oversaturated colors, overexposed, static, blurry details, subtitles, artwork, painting, still picture, overall gray, \
     worst quality, low quality, jpeg artifacts, ugly, incomplete, extra fingers, poorly drawn hands, poorly drawn faces, deformed, disfigured, \
     malformed limbs, fused fingers, motionless frame, cluttered background, three legs, many people in the background, walking backwards";

/// Wan 2.2 image-to-video's files and clip settings. The field names are the ones Image Sage saves, so its settings can be copied.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct ModelFiles {
    /// The low-noise model: it finishes the clip that the high-noise model starts.
    pub diffusion_model: String,
    /// The high-noise model, which makes the first sampling steps.
    pub high_noise_model: String,
    pub text_encoder: String,
    pub vae: String,
    /// What every clip avoids.
    pub negative_prompt: String,
    /// An optional LoRA file for the low-noise model; empty means none.
    pub lora: String,
    /// How strongly the LoRA applies: 1 is full strength, 0 turns it off.
    pub lora_strength: f64,
    /// An optional LoRA file for the high-noise model; empty means none.
    pub high_noise_lora: String,
    pub high_noise_lora_strength: f64,
}

impl Default for ModelFiles {
    fn default() -> Self {
        Self {
            diffusion_model: String::new(),
            high_noise_model: String::new(),
            text_encoder: String::new(),
            vae: String::new(),
            negative_prompt: DEFAULT_NEGATIVE_PROMPT.into(),
            lora: String::new(),
            lora_strength: 1.0,
            high_noise_lora: String::new(),
            high_noise_lora_strength: 1.0,
        }
    }
}

impl ModelFiles {
    /// The model files that are not set or do not exist, by name. A LoRA is optional, so it counts only when set.
    pub fn missing(&self) -> Vec<&'static str> {
        let mut missing: Vec<&'static str> = [
            (&self.diffusion_model, "low-noise model"),
            (&self.high_noise_model, "high-noise model"),
            (&self.text_encoder, "text encoder"),
            (&self.vae, "VAE"),
        ]
        .into_iter()
        .filter(|(path, _)| path.trim().is_empty() || !Path::new(path.as_str()).is_file())
        .map(|(_, name)| name)
        .collect();
        let lost = |path: &str| !path.trim().is_empty() && !Path::new(path.trim()).is_file();
        if lost(&self.lora) {
            missing.push("low-noise LoRA");
        }
        if lost(&self.high_noise_lora) {
            missing.push("high-noise LoRA");
        }
        missing
    }

    /**
     * The folder the engine reads LoRAs from, when a LoRA is set. The engine
     * has one LoRA folder: for two LoRAs in different folders, it is the
     * nearest folder that holds both.
     */
    fn lora_dir(&self) -> Option<PathBuf> {
        let folders: Vec<&Path> = [&self.lora, &self.high_noise_lora]
            .into_iter()
            .filter(|path| !path.trim().is_empty())
            .filter_map(|path| Path::new(path.trim()).parent())
            .collect();
        let shared = folders.first()?.ancestors().find(|folder| folders.iter().all(|other| other.starts_with(folder)))?;
        Some(shared.to_path_buf())
    }

    /// A LoRA's name for the engine: its path from the LoRA folder, with forward slashes. None when the LoRA is not set.
    fn lora_name(&self, lora: &str) -> Option<String> {
        if lora.trim().is_empty() {
            return None;
        }
        let inside = Path::new(lora.trim()).strip_prefix(self.lora_dir()?).ok()?;
        Some(inside.components().map(|part| part.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"))
    }

    /// The LoRA list of a job: the low-noise model's LoRA, then the high-noise model's. A LoRA of strength 0 is left out.
    fn job_loras(&self) -> Vec<Value> {
        [(&self.lora, self.lora_strength, false), (&self.high_noise_lora, self.high_noise_lora_strength, true)]
            .into_iter()
            .filter(|(_, strength, _)| *strength != 0.0)
            .filter_map(|(lora, strength, high_noise)| {
                let name = self.lora_name(lora)?;
                Some(if high_noise {
                    json!({ "path": name, "multiplier": strength, "is_high_noise": true })
                } else {
                    json!({ "path": name, "multiplier": strength })
                })
            })
            .collect()
    }

    /// The engine's start settings: its model files and the LoRA folder. The LoRAs and their strength apply per job.
    fn same_start(&self, other: &ModelFiles) -> bool {
        self.diffusion_model == other.diffusion_model
            && self.high_noise_model == other.high_noise_model
            && self.text_encoder == other.text_encoder
            && self.vae == other.vae
            && self.lora_dir() == other.lora_dir()
    }
}

#[derive(Serialize, Default, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FoundModels {
    diffusion_model: Option<String>,
    high_noise_model: Option<String>,
    text_encoder: Option<String>,
    vae: Option<String>,
}

/// Finds Wan 2.2 image-to-video's files in a ComfyUI folder (its root, or its `models` folder).
#[tauri::command]
pub fn find_models(folder: String) -> FoundModels {
    find_in_folder(Path::new(&folder))
}

fn find_in_folder(folder: &Path) -> FoundModels {
    let Some(root) = [folder.to_path_buf(), folder.join("models"), folder.join("ComfyUI").join("models")]
        .into_iter()
        .find(|candidate| ["diffusion_models", "unet", "text_encoders", "clip", "vae"].iter().any(|name| candidate.join(name).is_dir()))
    else {
        return FoundModels::default();
    };
    let model_file = |name: &str| name.ends_with(".safetensors") || name.ends_with(".gguf") || name.ends_with(".sft");
    /* Each Wan 2.2 model has a low-noise and a high-noise file, with `i2v` in the names. */
    let part = |noise: &'static str| {
        best_file(&root, &["diffusion_models", "unet", "checkpoints"], move |name| {
            let plain = name.replace(['_', '-'], "");
            (model_file(name) && plain.contains("wan2.2") && plain.contains("i2v") && plain.contains(noise)).then_some(0)
        })
    };
    FoundModels {
        diffusion_model: part("lownoise"),
        high_noise_model: part("highnoise"),
        text_encoder: best_file(&root, &["text_encoders", "clip"], |name| {
            (model_file(name) && name.contains("umt5")).then(|| if name.contains("fp16") { 0 } else if name.contains("fp8") { 1 } else { 2 })
        }),
        vae: best_file(&root, &["vae"], |name| (model_file(name) && name.contains("wan_2.1_vae")).then_some(0)),
    }
}

/// The best-ranked file (lowest rank) in the named folders and their subfolders. `rank` gets the lowercase file name.
fn best_file(root: &Path, folders: &[&str], rank: impl Fn(&str) -> Option<u32>) -> Option<String> {
    let mut best: Option<(u32, PathBuf)> = None;
    let mut pending: Vec<(PathBuf, u32)> = folders.iter().map(|folder| (root.join(folder), 0)).collect();
    while let Some((dir, depth)) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if depth < 2 {
                    pending.push((path, depth + 1));
                }
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if let Some(score) = rank(&name) {
                if best.as_ref().is_none_or(|(current, _)| score < *current) {
                    best = Some((score, path));
                }
            }
        }
    }
    best.map(|(_, path)| path.to_string_lossy().into_owned())
}

/// Image Sage's saved Wan 2.2 image-to-video settings, when Image Sage has them.
pub fn image_sage_files(app: &AppHandle) -> Option<ModelFiles> {
    let path = app.path().config_dir().ok()?.join(IMAGE_SAGE_FOLDER).join("local-engine.json");
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    serde_json::from_value(saved["models"]["wan-2.2-i2v"].clone()).ok()
}

/// The last lines the engine printed, to explain a failed start.
#[derive(Default)]
struct ServerLog(VecDeque<String>);

impl ServerLog {
    fn record(&mut self, line: &str) {
        let line = line.trim_end_matches("\u{1b}[K").trim();
        /* Progress bars redraw one line many times; they explain nothing. */
        if line.is_empty() || line.starts_with('|') {
            return;
        }
        if self.0.len() == LOG_LINES {
            self.0.pop_front();
        }
        self.0.push_back(line.to_string());
    }

    fn last_error(&self) -> Option<String> {
        self.0.iter().rev().find(|line| line.starts_with("[E]")).or_else(|| self.0.back()).cloned()
    }
}

struct Server {
    child: Child,
    port: u16,
    files: ModelFiles,
    ready: bool,
    log: Arc<Mutex<ServerLog>>,
}

/// The running engine, if any.
#[derive(Default, Clone)]
pub struct Engine(Arc<Mutex<Option<Server>>>);

impl Engine {
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Server>> {
        self.0.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// Ends the engine process. This removes the model from the GPU memory.
    pub fn stop(&self) {
        if let Some(mut server) = self.lock().take() {
            let _ = server.child.kill();
            let _ = server.child.wait();
        }
    }

    pub fn loaded(&self) -> bool {
        self.lock().as_ref().is_some_and(|server| server.ready)
    }
}

fn engine_installed(dir: &Path) -> bool {
    dir.join(ENGINE_MARKER).is_file() && dir.join(SERVER_EXE).is_file()
}

fn own_engines_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_local_data_dir()
        .map_err(|error| format!("Could not find the app data folder: {error}"))?
        .join("engines");
    std::fs::create_dir_all(&dir).map_err(|error| format!("Could not create the engines folder: {error}"))?;
    Ok(dir)
}

/// The installed engine's folder: this app's own copy, or the copy Image Sage installed.
fn installed_engine_dir(app: &AppHandle) -> Result<Option<PathBuf>, String> {
    let own = own_engines_dir(app)?.join(ENGINE_BUILD);
    if engine_installed(&own) {
        return Ok(Some(own));
    }
    let shared = app.path().local_data_dir().ok().map(|dir| dir.join(IMAGE_SAGE_FOLDER).join("engines").join(ENGINE_BUILD));
    Ok(shared.filter(|dir| engine_installed(dir)))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineStatus {
    installed: bool,
    /// The engine is the copy that Image Sage installed.
    shared_with_image_sage: bool,
    size_bytes: u64,
    /// The model is in the GPU memory now.
    loaded: bool,
}

#[tauri::command]
pub fn engine_status(app: AppHandle, engine: tauri::State<'_, Engine>) -> Result<EngineStatus, String> {
    let dir = installed_engine_dir(&app)?;
    Ok(EngineStatus {
        shared_with_image_sage: dir.as_ref().is_some_and(|dir| !dir.starts_with(own_engines_dir(&app).unwrap_or_default())),
        installed: dir.is_some(),
        size_bytes: ENGINE_ZIPS.iter().map(|zip| zip.size).sum(),
        loaded: engine.loaded(),
    })
}

fn client(timeout: Duration) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(concat!("ImageSageVideo/", env!("CARGO_PKG_VERSION")))
        .timeout(timeout)
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .map_err(|error| format!("Could not start the network client: {error}"))
}

fn describe_transport_error(service: &str, error: &reqwest::Error) -> String {
    if error.is_timeout() {
        return format!("{service} did not answer in time. Try again.");
    }
    if error.is_connect() {
        return format!("Could not reach {service}.");
    }
    /* reqwest's own message hides the real reason, so the causes are joined to it. */
    let mut text = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    format!("The connection to {service} failed: {text}")
}

fn emit_download(app: &AppHandle, stage: &str, progress: f64) {
    let _ = app.emit("engine-download", json!({ "stage": stage, "progress": progress.clamp(0.0, 1.0) }));
}

/**
 * Downloads one engine zip to `target`, through a `.part` file that becomes
 * the target only when its size and hash match.
 */
async fn download_verified(app: &AppHandle, client: &reqwest::Client, zip: &EngineZip, target: &Path, done: u64, total: u64) -> Result<(), String> {
    let mut partial = target.as_os_str().to_owned();
    partial.push(".part");
    let partial = PathBuf::from(partial);
    let response = client.get(zip.url).send().await.map_err(|error| describe_transport_error("GitHub", &error))?;
    if !response.status().is_success() {
        return Err(format!("The engine download failed: the server returned {}.", response.status()));
    }
    let mut output = tokio::fs::File::create(&partial).await.map_err(|error| format!("Could not create the engine file: {error}"))?;
    let mut hasher = Sha256::new();
    let mut stream = response.bytes_stream();
    let mut downloaded = 0_u64;
    let mut reported = 0_u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| describe_transport_error("GitHub", &error))?;
        hasher.update(&chunk);
        output.write_all(&chunk).await.map_err(|error| format!("Could not write the engine file: {error}"))?;
        downloaded += chunk.len() as u64;
        if downloaded - reported >= 1 << 20 {
            reported = downloaded;
            emit_download(app, "downloading", (done + downloaded) as f64 / total.max(1) as f64);
        }
    }
    output.flush().await.map_err(|error| format!("Could not write the engine file: {error}"))?;
    drop(output);
    let actual: String = hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect();
    if downloaded != zip.size || !actual.eq_ignore_ascii_case(zip.sha256) {
        let _ = tokio::fs::remove_file(&partial).await;
        return Err("The downloaded engine is damaged. Try again.".into());
    }
    tokio::fs::rename(&partial, target).await.map_err(|error| format!("Could not save the engine file: {error}"))
}

fn extract_zip(zip: &Path, target: &Path) -> Result<(), String> {
    let fail = |error: &dyn std::fmt::Display| format!("Could not unpack the engine: {error}");
    let file = std::fs::File::open(zip).map_err(|error| fail(&error))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| fail(&error))?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| fail(&error))?;
        let relative = entry.enclosed_name().ok_or_else(|| fail(&"a file name points outside the folder"))?;
        let path = target.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&path).map_err(|error| fail(&error))?;
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| fail(&error))?;
        }
        let mut output = std::fs::File::create(&path).map_err(|error| fail(&error))?;
        std::io::copy(&mut entry, &mut output).map_err(|error| fail(&error))?;
    }
    Ok(())
}

/// Downloads and unpacks the engine into this app's own folder, checking each zip's hash before it is used.
#[tauri::command]
pub async fn engine_download(app: AppHandle) -> Result<(), String> {
    if installed_engine_dir(&app)?.is_some() {
        return Ok(());
    }
    if ENGINE_ZIPS.iter().any(|zip| !(zip.url.starts_with(ENGINE_URL_PREFIX) && zip.url.ends_with(".zip"))) {
        return Err("The engine URL is not an official stable-diffusion.cpp release.".into());
    }
    let engines = own_engines_dir(&app)?;
    let target = engines.join(ENGINE_BUILD);
    let total: u64 = ENGINE_ZIPS.iter().map(|zip| zip.size).sum();
    emit_download(&app, "downloading", 0.0);
    let client = client(DOWNLOAD_TIMEOUT)?;
    let mut done = 0_u64;
    let mut zips = Vec::new();
    for zip in &ENGINE_ZIPS {
        let path = engines.join(zip.name);
        let complete = std::fs::metadata(&path).map(|meta| meta.len() == zip.size).unwrap_or(false);
        if !complete {
            download_verified(&app, &client, zip, &path, done, total).await?;
        }
        done += zip.size;
        zips.push(path);
    }
    emit_download(&app, "preparing", 1.0);
    tokio::task::spawn_blocking(move || {
        let _ = std::fs::remove_dir_all(&target);
        for zip in &zips {
            extract_zip(zip, &target)?;
        }
        std::fs::write(target.join(ENGINE_MARKER), json!({ "build": ENGINE_BUILD }).to_string())
            .map_err(|error| format!("Could not finish the engine install: {error}"))?;
        for zip in &zips {
            let _ = std::fs::remove_file(zip);
        }
        Ok::<(), String>(())
    })
    .await
    .map_err(|error| format!("The engine install stopped unexpectedly: {error}"))?
}

/// Reads one output stream of the engine so it never blocks, keeping the last lines.
fn pump(mut stream: impl Read + Send + 'static, log: Arc<Mutex<ServerLog>>) {
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        let mut line = Vec::new();
        while let Ok(count) = stream.read(&mut buffer) {
            if count == 0 {
                break;
            }
            for &byte in &buffer[..count] {
                if byte == b'\r' || byte == b'\n' {
                    if !line.is_empty() {
                        log.lock().unwrap_or_else(|error| error.into_inner()).record(&String::from_utf8_lossy(&line));
                        line.clear();
                    }
                } else {
                    line.push(byte);
                }
            }
        }
    });
}

fn free_port() -> Result<u16, String> {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .map_err(|error| format!("Could not find a free port for the local engine: {error}"))
}

/// Starts the engine with Wan 2.2 image-to-video. The start flags are the ones Image Sage uses.
fn spawn_server(dir: &Path, files: &ModelFiles) -> Result<Server, String> {
    let port = free_port()?;
    let mut command = Command::new(dir.join(SERVER_EXE));
    command
        .current_dir(dir)
        .args(["--listen-ip", "127.0.0.1", "--listen-port", &port.to_string()])
        .args(["--diffusion-model", &files.diffusion_model])
        .args(["--t5xxl", &files.text_encoder])
        .args(["--vae", &files.vae])
        .args(["--high-noise-diffusion-model", &files.high_noise_model])
        /*
         * Wan 2.2's two models are larger than the GPU memory together, so the
         * engine keeps part of them outside it. Its default way to apply a LoRA
         * (into the weights, at once) then stops the engine with an error; at
         * run time it works.
         */
        .args(["--lora-apply-mode", "at_runtime"])
        /* The conditioning cache can hand a later job an earlier prompt's encoding, so it is off. */
        .args(["--conditioning-cache-size", "0"]);
    if let Some(dir) = files.lora_dir() {
        command.arg("--lora-model-dir").arg(dir);
    }
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().map_err(|error| format!("Could not start the local engine: {error}"))?;
    #[cfg(windows)]
    stop_with_app(&child);
    let log = Arc::new(Mutex::new(ServerLog::default()));
    if let Some(stdout) = child.stdout.take() {
        pump(stdout, log.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        pump(stderr, log.clone());
    }
    Ok(Server { child, port, files: files.clone(), ready: false, log })
}

/// Puts the engine in a job object that Windows closes with this app, so the engine never outlives the app.
#[cfg(windows)]
fn stop_with_app(child: &Child) {
    use std::os::windows::io::AsRawHandle;
    use std::sync::OnceLock;
    use windows::Win32::{Foundation::HANDLE, System::JobObjects::*};

    static JOB: OnceLock<Option<usize>> = OnceLock::new();
    let job = JOB.get_or_init(|| unsafe {
        let job = CreateJobObjectW(None, windows::core::PCWSTR::null()).ok()?;
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const std::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
        .ok()?;
        Some(job.0 as usize)
    });
    if let Some(job) = job {
        unsafe {
            let _ = AssignProcessToJobObject(HANDLE(*job as *mut _), HANDLE(child.as_raw_handle()));
        }
    }
}

/// What a clip job is doing now.
pub enum Step<'a> {
    /// The engine starts and loads the model.
    Loading,
    /// A new sampling step: how many of the clip's steps are done, and a small PNG of the clip so far, as base64.
    Sampling { step: u64, preview: Option<&'a str> },
    /// Sampling is done; the engine decodes and encodes the clip.
    Finishing,
}

/**
 * Starts the engine with these files, or reuses the running one, and waits
 * until the model is loaded. An engine with other files stops first.
 */
async fn ready_port(app: &AppHandle, engine: &Engine, files: &ModelFiles, on_step: &mut (impl FnMut(Step<'_>) + Send)) -> Result<u16, String> {
    let dir = installed_engine_dir(app)?.ok_or("The local engine is not installed. Download it in Settings.")?;
    let missing = files.missing();
    if !missing.is_empty() {
        return Err(format!("Choose the Wan 2.2 {} in Settings.", missing.join(", ")));
    }
    let port = {
        let mut slot = engine.lock();
        let reusable = slot.as_mut().is_some_and(|server| server.files.same_start(files) && matches!(server.child.try_wait(), Ok(None)));
        if !reusable {
            if let Some(mut old) = slot.take() {
                let _ = old.child.kill();
                let _ = old.child.wait();
            }
            *slot = Some(spawn_server(&dir, files)?);
        }
        let server = slot.as_ref().expect("the engine was just started");
        if server.ready {
            return Ok(server.port);
        }
        server.port
    };
    on_step(Step::Loading);
    let client = client(REQUEST_TIMEOUT)?;
    let url = format!("http://127.0.0.1:{port}/sdcpp/v1/capabilities");
    let started = Instant::now();
    loop {
        if let Some(error) = stopped_reason(engine, port, "") {
            return Err(error);
        }
        if engine.lock().as_ref().is_none_or(|server| server.port != port) {
            return Err("The local engine was stopped.".into());
        }
        if client.get(&url).send().await.is_ok_and(|response| response.status().is_success()) {
            if let Some(server) = engine.lock().as_mut().filter(|server| server.port == port) {
                server.ready = true;
            }
            return Ok(port);
        }
        if started.elapsed() > LOAD_TIMEOUT {
            return Err("The local engine did not load the model in 10 minutes.".into());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// When the engine process has ended, the reason: the last line it printed. The ended engine is removed.
fn stopped_reason(engine: &Engine, port: u16, during: &str) -> Option<String> {
    let mut slot = engine.lock();
    let server = slot.as_mut().filter(|server| server.port == port)?;
    let status = server.child.try_wait().ok().flatten()?;
    let reason = server.log.lock().unwrap_or_else(|error| error.into_inner()).last_error();
    slot.take();
    Some(match reason {
        Some(reason) => format!("The local engine stopped{during} ({status}): {reason}"),
        None => format!("The local engine stopped{during} ({status})."),
    })
}

/**
 * Explains a failed job. When the engine stopped during the job, the reason
 * is the last line it printed, not the lost connection that the job saw.
 */
async fn job_error(engine: &Engine, port: u16, error: String) -> String {
    /* The connection closes a moment before Windows reports that the engine exited. */
    for _ in 0..10 {
        if let Some(reason) = stopped_reason(engine, port, " during the clip") {
            return reason;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    error
}

/// Asks the engine to stop a job when this app stops waiting for it (pause or error).
struct CancelOnDrop(Option<String>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(url) = self.0.take() {
            tauri::async_runtime::spawn(async move {
                if let Ok(client) = client(CONNECT_TIMEOUT) {
                    let _ = client.post(url).send().await;
                }
            });
        }
    }
}

/**
 * The engine's job request for one clip. Wan 2.2 samples with two models:
 * the high-noise model starts the clip and the low-noise model finishes it.
 * The engine divides the steps between them at `moe_boundary`. The flow
 * shift (5) and the boundary (0.9) are the model makers' values for a clip
 * that starts from a picture.
 */
fn video_body(prompt: &str, width: u32, height: u32, files: &ModelFiles, image: &str) -> Value {
    let sample_params = |steps: i64| {
        json!({
            "sample_method": "euler",
            "sample_steps": steps,
            "flow_shift": 5.0,
            "guidance": { "txt_cfg": CFG_SCALE }
        })
    };
    let mut body = json!({
        "prompt": prompt,
        "negative_prompt": files.negative_prompt.trim(),
        "width": width,
        "height": height,
        "seed": -1,
        "video_frames": CLIP_FRAMES,
        "fps": FPS,
        "moe_boundary": 0.9,
        "sample_params": sample_params(i64::from(VIDEO_STEPS)),
        /* -1 lets the engine take the high-noise steps from the boundary. */
        "high_noise_sample_params": sample_params(-1),
        "preview": "proj",
        "preview_interval": 1,
        "output_format": "webm",
        "init_image": image
    });
    let loras = files.job_loras();
    if !loras.is_empty() {
        body["lora"] = json!(loras);
    }
    body
}

/**
 * Joins a job's sampling passes into one count. The engine restarts `step`
 * at 1 in each pass; this adds the steps of the passes before.
 */
#[derive(Default)]
struct PassSteps {
    pass: u64,
    step: u64,
    /// The steps of the passes before this one.
    before: u64,
    /// The steps of this pass.
    total: u64,
}

impl PassSteps {
    /// The step over all passes up to now, or none when the engine shows the same step again.
    fn next(&mut self, pass: u64, step: u64, total: u64) -> Option<u64> {
        if total == 0 || (pass == self.pass && step == self.step) {
            return None;
        }
        if pass != self.pass {
            self.before += self.total;
            self.pass = pass;
        }
        self.step = step;
        self.total = total;
        Some(self.before + step)
    }
}

/**
 * Makes one 5-second clip from `image` (a PNG data URL at the clip size) and
 * returns its WebM bytes. The clip stays in memory; the caller decides where
 * it goes. If the caller stops waiting, the engine is told to cancel the job.
 */
pub async fn make_clip(
    app: &AppHandle,
    engine: &Engine,
    files: &ModelFiles,
    prompt: &str,
    (width, height): (u32, u32),
    image: &str,
    mut on_step: impl FnMut(Step<'_>) + Send,
) -> Result<Vec<u8>, String> {
    let port = ready_port(app, engine, files, &mut on_step).await?;
    on_step(Step::Sampling { step: 0, preview: None });
    let body = video_body(prompt.trim(), width, height, files, image);
    let result = match run_job(port, &body, &mut on_step).await {
        Ok(result) => result,
        Err(error) => return Err(job_error(engine, port, error).await),
    };
    let encoded = result["b64_json"].as_str().ok_or("The local engine returned no video.")?;
    STANDARD.decode(encoded).map_err(|error| format!("The local engine returned a video that is not valid: {error}"))
}

/// Gives the engine one clip job and waits for its result. It calls `on_step` once for each new sampling step.
async fn run_job(port: u16, body: &Value, on_step: &mut (impl FnMut(Step<'_>) + Send)) -> Result<Value, String> {
    let client = client(REQUEST_TIMEOUT)?;
    let base = format!("http://127.0.0.1:{port}/sdcpp/v1");
    let fail = |error: reqwest::Error| describe_transport_error("the local engine", &error);
    let created: Value = client.post(format!("{base}/vid_gen")).json(body).send().await.map_err(fail)?.json().await.map_err(fail)?;
    let id = created["id"].as_str().ok_or_else(|| format!("The local engine did not accept the clip: {created}"))?.to_string();
    let mut cancel = CancelOnDrop(Some(format!("{base}/jobs/{id}/cancel")));
    let mut passes = PassSteps::default();
    loop {
        tokio::time::sleep(POLL_INTERVAL).await;
        let mut job: Value = client.get(format!("{base}/jobs/{id}")).send().await.map_err(fail)?.json().await.map_err(fail)?;
        match job["status"].as_str() {
            Some("completed") => {
                cancel.0 = None;
                return Ok(job["result"].take());
            }
            Some("failed") | Some("cancelled") => {
                cancel.0 = None;
                let reason = job["error"]["message"].as_str().unwrap_or("no reason given");
                return Err(format!("The local engine could not make the clip: {reason}"));
            }
            _ => {
                let preview = &job["preview"];
                let (Some(step), Some(total)) = (preview["step"].as_u64(), preview["total_steps"].as_u64()) else { continue };
                let Some(step) = passes.next(preview["pass"].as_u64().unwrap_or(1), step, total) else { continue };
                if step >= u64::from(VIDEO_STEPS) {
                    on_step(Step::Finishing);
                } else {
                    on_step(Step::Sampling { step, preview: preview["b64_json"].as_str() });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clip_job_uses_both_wan_models_and_starts_from_the_picture() {
        let body = video_body("a fox runs", 832, 464, &ModelFiles::default(), "data:image/png;base64,AAAA");
        assert_eq!(body["video_frames"], 81);
        assert_eq!(body["fps"], 16);
        assert_eq!(body["output_format"], "webm");
        assert_eq!(body["init_image"], "data:image/png;base64,AAAA");
        assert_eq!(body["sample_params"]["sample_steps"], 20);
        assert_eq!(body["high_noise_sample_params"]["sample_steps"], -1);
        assert_eq!(body["moe_boundary"], 0.9);
        assert!(body.get("lora").is_none());
    }

    #[test]
    fn a_clip_job_has_a_lora_for_each_wan_model() {
        let files = ModelFiles {
            lora: "D:\\models\\loras\\wan\\Motion_Low.safetensors".into(),
            high_noise_lora: "D:\\models\\loras\\wan\\Motion_High.safetensors".into(),
            high_noise_lora_strength: 0.5,
            ..ModelFiles::default()
        };
        let body = video_body("x", 832, 464, &files, "data:image/png;base64,AAAA");
        assert_eq!(body["lora"][0], json!({ "path": "Motion_Low.safetensors", "multiplier": 1.0 }));
        assert_eq!(body["lora"][1], json!({ "path": "Motion_High.safetensors", "multiplier": 0.5, "is_high_noise": true }));
        let off = ModelFiles { lora_strength: 0.0, high_noise_lora_strength: 0.0, ..files };
        assert!(video_body("x", 832, 464, &off, "data:image/png;base64,AAAA").get("lora").is_none());
    }

    #[test]
    fn files_that_are_not_set_are_missing() {
        assert_eq!(ModelFiles::default().missing(), vec!["low-noise model", "high-noise model", "text encoder", "VAE"]);
    }

    #[test]
    fn image_sage_settings_fit_these_files() {
        let saved = json!({
            "diffusionModel": "D:\\m\\low.safetensors", "highNoiseModel": "D:\\m\\high.safetensors", "textEncoder": "D:\\m\\umt5.safetensors",
            "vae": "D:\\m\\vae.safetensors", "negativePrompt": "static", "promptPrefix": "", "sampler": "euler", "scheduler": "simple",
            "lora": "", "loraStrength": 1.0, "highNoiseLora": "", "highNoiseLoraStrength": 1.0
        });
        let files: ModelFiles = serde_json::from_value(saved).unwrap();
        assert_eq!(files.high_noise_model, "D:\\m\\high.safetensors");
        assert_eq!(files.negative_prompt, "static");
    }

    #[test]
    fn the_steps_of_two_passes_count_as_one() {
        let mut passes = PassSteps::default();
        assert_eq!(passes.next(1, 1, 2), Some(1));
        assert_eq!(passes.next(1, 1, 2), None);
        assert_eq!(passes.next(1, 2, 2), Some(2));
        assert_eq!(passes.next(2, 1, 18), Some(3));
        assert_eq!(passes.next(2, 18, 18), Some(20));
    }
}
