use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::{
    crypto, document,
    engine::{self, Engine, Step, VIDEO_STEPS},
    settings, tray, webm,
};

/**
 * The job: one document, and three 5-second clips for each chosen layer.
 * The job runs in the Rust side of the app, so it goes on while the window
 * is hidden in the tray. The clips are made one after the other, bottom
 * layer first.
 *
 * Pause stops the clip that is in work and ends the engine, which removes
 * the model from the GPU memory. The engine cannot keep half a clip, so
 * Resume makes that clip again from its start.
 *
 * The job list is kept in an encrypted file after each change, so a job
 * that was not finished when the app closed can go on later. A clip is in
 * memory until it is finished; then it is written to the output folder as
 * a normal WebM file.
 */
pub const CLIPS_PER_LAYER: usize = 3;
/// After this many clips fail one after the other, the job pauses; the cause is then not one bad clip.
const MAX_FAILURES_IN_A_ROW: u32 = 2;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum ClipStatus {
    Pending,
    Running,
    Done,
    Failed,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Clip {
    status: ClipStatus,
    /// The finished clip's file.
    file: Option<String>,
    error: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LayerJob {
    /// The layer's place in the document, from 0 at the bottom.
    index: usize,
    tile: String,
    name: String,
    prompt: String,
    clips: Vec<Clip>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    document: String,
    output_dir: String,
    /// Also make one video of all the clips, joined.
    combine: bool,
    clip_width: u32,
    clip_height: u32,
    layers: Vec<LayerJob>,
    /// The combined video's file, once it is made.
    combined: Option<String>,
}

impl Job {
    fn next_pending(&self) -> Option<(usize, usize)> {
        self.layers.iter().enumerate().find_map(|(layer, job)| {
            job.clips.iter().position(|clip| clip.status == ClipStatus::Pending).map(|clip| (layer, clip))
        })
    }

    fn count(&self, status: ClipStatus) -> usize {
        self.layers.iter().flat_map(|layer| &layer.clips).filter(|clip| clip.status == status).count()
    }

    fn total(&self) -> usize {
        self.layers.iter().map(|layer| layer.clips.len()).sum()
    }

    /// The finished clips' files, in the order of the combined video: bottom layer first.
    fn finished_files(&self) -> Vec<String> {
        self.layers
            .iter()
            .flat_map(|layer| &layer.clips)
            .filter(|clip| clip.status == ClipStatus::Done)
            .filter_map(|clip| clip.file.clone())
            .collect()
    }
}

#[derive(Serialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum RunState {
    /// There is no job.
    Idle,
    Running,
    Paused,
    /// Every clip is done or failed.
    Finished,
}

/// What the job does now.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Current {
    layer: usize,
    clip: usize,
    /// "preparing", "loading", "generating", "finishing", or "joining".
    stage: &'static str,
    /// The part of this clip that is done, from 0 to 1.
    progress: f64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    job: Option<Job>,
    state: RunState,
    current: Option<Current>,
    /// Why the job paused by itself, or why the combined video could not be made.
    error: Option<String>,
    clips_done: usize,
    clips_failed: usize,
    clips_total: usize,
}

struct Inner {
    job: Option<Job>,
    state: RunState,
    current: Option<Current>,
    error: Option<String>,
    task: Option<tauri::async_runtime::JoinHandle<()>>,
    /// Goes up at each start, pause and clear. A runner may change the job only while its own number is the current one.
    generation: u64,
}

#[derive(Clone)]
pub struct Queue(Arc<Mutex<Inner>>);

impl Queue {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(|error| error.into_inner())
    }

    pub fn is_running(&self) -> bool {
        self.lock().state == RunState::Running
    }
}

impl Inner {
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            job: self.job.clone(),
            state: self.state,
            current: self.current.clone(),
            error: self.error.clone(),
            clips_done: self.job.as_ref().map_or(0, |job| job.count(ClipStatus::Done)),
            clips_failed: self.job.as_ref().map_or(0, |job| job.count(ClipStatus::Failed)),
            clips_total: self.job.as_ref().map_or(0, Job::total),
        }
    }
}

fn job_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_local_data_dir().map_err(|error| format!("Could not find the app data folder: {error}"))?;
    std::fs::create_dir_all(&dir).map_err(|error| format!("Could not create the app data folder: {error}"))?;
    Ok(dir.join("job.bin"))
}

/// Writes the job list to its encrypted file, or removes the file when there is no job.
fn save_job(app: &AppHandle, job: Option<&Job>) {
    let result = job_path(app).and_then(|path| match job {
        Some(job) => crypto::write_sealed(&path, &serde_json::to_vec(job).map_err(|error| error.to_string())?),
        None => match std::fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.to_string()),
            _ => Ok(()),
        },
    });
    if let Err(error) = result {
        eprintln!("[imagesagevideo] could not save the job list: {error}");
    }
}

/// The job that was saved when the app closed. A clip that was in work then starts again.
pub fn restore(app: &AppHandle) -> Queue {
    let mut job = job_path(app)
        .and_then(|path| crypto::read_sealed(&path))
        .ok()
        .flatten()
        .and_then(|bytes| serde_json::from_slice::<Job>(&bytes).ok());
    if let Some(job) = job.as_mut() {
        for clip in job.layers.iter_mut().flat_map(|layer| &mut layer.clips) {
            if clip.status == ClipStatus::Running {
                clip.status = ClipStatus::Pending;
            }
        }
    }
    let state = match &job {
        None => RunState::Idle,
        Some(job) if job.next_pending().is_some() => RunState::Paused,
        Some(_) => RunState::Finished,
    };
    Queue(Arc::new(Mutex::new(Inner { job, state, current: None, error: None, task: None, generation: 0 })))
}

/// Tells the window and the tray icon what the job does now.
fn publish(app: &AppHandle, inner: &Inner) {
    let snapshot = inner.snapshot();
    let state = match snapshot.state {
        RunState::Idle => tray::TrayState::Idle,
        RunState::Running => tray::TrayState::Working,
        RunState::Paused => tray::TrayState::Paused,
        RunState::Finished => tray::TrayState::Finished,
    };
    tray::update(app, &tray_text(&snapshot), state);
    let _ = app.emit("queue-changed", snapshot);
}

/// Shows the job's state on the tray icon; used once the tray icon exists.
pub fn show_in_tray(app: &AppHandle) {
    publish(app, &app.state::<Queue>().lock());
}

fn tray_text(snapshot: &Snapshot) -> String {
    let counts = format!("{} of {} clips", snapshot.clips_done, snapshot.clips_total);
    match snapshot.state {
        RunState::Idle => "Image Sage Video".into(),
        RunState::Running => match &snapshot.current {
            Some(current) if current.stage == "joining" => "Image Sage Video: joining the clips".into(),
            Some(current) => format!("Image Sage Video: {counts} done, this clip {}%", (current.progress * 100.0).round()),
            None => format!("Image Sage Video: {counts} done"),
        },
        RunState::Paused => format!("Image Sage Video: paused, {counts} done"),
        RunState::Finished => format!("Image Sage Video: finished, {counts} done"),
    }
}

/// A file name part from a layer's name: letters, digits, spaces and dashes only.
fn slug(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|character| if character.is_alphanumeric() || character == '-' { character } else { ' ' })
        .collect();
    let joined = cleaned.split_whitespace().collect::<Vec<_>>().join("-");
    let short: String = joined.chars().take(40).collect();
    let short = short.trim_matches('-').to_string();
    if short.is_empty() { "layer".into() } else { short }
}

/// `name.webm` in `dir`, or `name-2.webm` and so on, so that no file is replaced.
fn unused_path(dir: &Path, name: &str) -> PathBuf {
    let mut path = dir.join(format!("{name}.webm"));
    let mut number = 2;
    while path.exists() {
        path = dir.join(format!("{name}-{number}.webm"));
        number += 1;
    }
    path
}

fn document_stem(document: &str) -> String {
    Path::new(document).file_stem().map(|stem| slug(&stem.to_string_lossy())).unwrap_or_else(|| "document".into())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRequest {
    document: String,
    output_dir: String,
    combine: bool,
    /// The layers to make clips of, by their file in the document.
    tiles: Vec<String>,
}

#[tauri::command]
pub fn queue_get(queue: State<'_, Queue>) -> Snapshot {
    queue.lock().snapshot()
}

/// Starts a new job. It replaces a job that is paused or finished.
#[tauri::command]
pub async fn queue_start(app: AppHandle, queue: State<'_, Queue>, engine: State<'_, Engine>, request: StartRequest) -> Result<(), String> {
    if queue.is_running() {
        return Err("A job is running. Pause it first.".into());
    }
    let output_dir = crate::files::windows_path(&request.output_dir);
    let output = PathBuf::from(&output_dir);
    if output_dir.is_empty() || !output.is_absolute() {
        return Err("Choose the folder for the videos first.".into());
    }
    std::fs::create_dir_all(&output).map_err(|error| format!("Could not use the folder {}: {error}", output.display()))?;
    let missing = settings::load(&app).files.missing();
    if !missing.is_empty() {
        return Err(format!("Choose the Wan 2.2 {} in Settings.", missing.join(", ")));
    }
    let document_path = request.document.clone();
    let (layers, size) = tauri::async_runtime::spawn_blocking(move || {
        let layers = document::read_layers(Path::new(&document_path))?;
        let size = document::document_clip_size(Path::new(&document_path), &layers)?;
        Ok::<_, String>((layers, size))
    })
    .await
    .map_err(|error| format!("Reading the document stopped unexpectedly: {error}"))??;
    let jobs: Vec<LayerJob> = layers
        .into_iter()
        .enumerate()
        .filter(|(_, layer)| request.tiles.contains(&layer.tile))
        .map(|(index, layer)| LayerJob {
            index,
            tile: layer.tile,
            name: layer.name,
            prompt: layer.prompt,
            clips: vec![Clip { status: ClipStatus::Pending, file: None, error: None }; CLIPS_PER_LAYER],
        })
        .collect();
    if jobs.is_empty() {
        return Err("Choose one layer or more first.".into());
    }
    let job = Job {
        document: crate::files::windows_path(&request.document),
        output_dir: output.to_string_lossy().into_owned(),
        combine: request.combine,
        clip_width: size.0,
        clip_height: size.1,
        layers: jobs,
        combined: None,
    };
    let mut saved = settings::load(&app);
    saved.output_dir = job.output_dir.clone();
    saved.combine = job.combine;
    settings::save(&app, &saved)?;
    let mut inner = queue.lock();
    if inner.state == RunState::Running {
        return Err("A job is running. Pause it first.".into());
    }
    inner.job = Some(job);
    spawn_runner(&app, &queue, &engine, &mut inner);
    Ok(())
}

/// Starts the runner for the job in `inner`; the caller holds the lock.
fn spawn_runner(app: &AppHandle, queue: &Queue, engine: &Engine, inner: &mut Inner) {
    inner.generation += 1;
    inner.state = RunState::Running;
    inner.error = None;
    inner.current = None;
    let (app_handle, queue_handle, engine_handle, generation) = (app.clone(), queue.clone(), engine.clone(), inner.generation);
    inner.task = Some(tauri::async_runtime::spawn(run(app_handle, queue_handle, engine_handle, generation)));
    save_job(app, inner.job.as_ref());
    publish(app, inner);
}

/// Stops the clip in work and ends the engine, so the model leaves the GPU memory. Resume makes that clip again.
pub fn pause(app: &AppHandle, queue: &Queue, engine: &Engine) {
    {
        let mut inner = queue.lock();
        if inner.state != RunState::Running {
            return;
        }
        inner.generation += 1;
        if let Some(task) = inner.task.take() {
            task.abort();
        }
        if let Some(job) = inner.job.as_mut() {
            for clip in job.layers.iter_mut().flat_map(|layer| &mut layer.clips) {
                if clip.status == ClipStatus::Running {
                    clip.status = ClipStatus::Pending;
                }
            }
        }
        inner.state = RunState::Paused;
        inner.current = None;
        save_job(app, inner.job.as_ref());
        publish(app, &inner);
    }
    engine.stop();
}

/// Goes on with a paused job, or makes a finished job's failed clips again.
pub fn resume(app: &AppHandle, queue: &Queue, engine: &Engine) {
    let mut inner = queue.lock();
    if inner.state == RunState::Running {
        return;
    }
    let Some(job) = inner.job.as_mut() else { return };
    for clip in job.layers.iter_mut().flat_map(|layer| &mut layer.clips) {
        if clip.status == ClipStatus::Failed {
            *clip = Clip { status: ClipStatus::Pending, file: None, error: None };
        }
    }
    spawn_runner(app, queue, engine, &mut inner);
}

#[tauri::command]
pub fn queue_pause(app: AppHandle, queue: State<'_, Queue>, engine: State<'_, Engine>) {
    pause(&app, &queue, &engine);
}

#[tauri::command]
pub fn queue_resume(app: AppHandle, queue: State<'_, Queue>, engine: State<'_, Engine>) {
    resume(&app, &queue, &engine);
}

/// Removes the job from the list. The finished videos stay in their folder.
#[tauri::command]
pub fn queue_clear(app: AppHandle, queue: State<'_, Queue>) -> Result<(), String> {
    let mut inner = queue.lock();
    if inner.state == RunState::Running {
        return Err("A job is running. Pause it first.".into());
    }
    inner.generation += 1;
    inner.job = None;
    inner.state = RunState::Idle;
    inner.current = None;
    inner.error = None;
    save_job(&app, None);
    publish(&app, &inner);
    Ok(())
}

/// Changes the job as the runner with this generation. None when that runner was paused or replaced; it must then stop.
fn update<T>(app: &AppHandle, queue: &Queue, generation: u64, save: bool, change: impl FnOnce(&mut Inner) -> T) -> Option<T> {
    let mut inner = queue.lock();
    if inner.generation != generation {
        return None;
    }
    let result = change(&mut inner);
    if save {
        save_job(app, inner.job.as_ref());
    }
    publish(app, &inner);
    Some(result)
}

/// Makes the job's clips, one after the other, then the combined video.
async fn run(app: AppHandle, queue: Queue, engine: Engine, generation: u64) {
    let mut picture: Option<(usize, String)> = None;
    let mut failures_in_a_row = 0_u32;
    loop {
        let next = update(&app, &queue, generation, true, |inner| {
            let job = inner.job.as_mut()?;
            let (layer, clip) = job.next_pending()?;
            job.layers[layer].clips[clip].status = ClipStatus::Running;
            let work = (layer, clip, job.clone());
            inner.current = Some(Current { layer, clip, stage: "preparing", progress: 0.0 });
            Some(work)
        });
        let Some(next) = next else { return };
        let Some((layer, clip, job)) = next else { break };
        let made = make_one(&app, &queue, &engine, generation, &job, layer, clip, &mut picture).await;
        failures_in_a_row = if made.is_ok() { 0 } else { failures_in_a_row + 1 };
        let stop = failures_in_a_row >= MAX_FAILURES_IN_A_ROW;
        let updated = update(&app, &queue, generation, true, |inner| {
            let Some(job) = inner.job.as_mut() else { return };
            job.layers[layer].clips[clip] = match &made {
                Ok(file) => Clip { status: ClipStatus::Done, file: Some(file.clone()), error: None },
                Err(error) => Clip { status: ClipStatus::Failed, file: None, error: Some(error.clone()) },
            };
            if stop {
                inner.state = RunState::Paused;
                inner.current = None;
                inner.task = None;
                inner.generation += 1;
                inner.error = made.as_ref().err().map(|error| format!("The job paused because {MAX_FAILURES_IN_A_ROW} clips failed one after the other. {error}"));
            }
        });
        if updated.is_none() {
            return;
        }
        if stop {
            engine.stop();
            return;
        }
    }
    /* Every clip is done or failed: the engine ends, so the model leaves the GPU memory. */
    engine.stop();
    let combine = update(&app, &queue, generation, false, |inner| {
        let job = inner.job.as_ref()?;
        let files = job.finished_files();
        if !job.combine || files.is_empty() {
            return None;
        }
        inner.current = Some(Current { layer: 0, clip: 0, stage: "joining", progress: 0.0 });
        let target = job.combined.clone().map(PathBuf::from).unwrap_or_else(|| unused_path(Path::new(&job.output_dir), &format!("{}-combined", document_stem(&job.document))));
        Some((files, target))
    });
    let Some(combine) = combine else { return };
    let joined = match combine {
        Some((files, target)) => Some(
            tauri::async_runtime::spawn_blocking(move || join_files(&files, &target))
                .await
                .unwrap_or_else(|error| Err(format!("Joining the clips stopped unexpectedly: {error}"))),
        ),
        None => None,
    };
    update(&app, &queue, generation, true, |inner| {
        match joined {
            Some(Ok(file)) => {
                if let Some(job) = inner.job.as_mut() {
                    job.combined = Some(file);
                }
            }
            Some(Err(error)) => inner.error = Some(format!("The combined video was not made. {error}")),
            None => {}
        }
        inner.state = RunState::Finished;
        inner.current = None;
        inner.task = None;
    });
}

/// Reads the finished clips and writes them, joined, to `target`.
fn join_files(files: &[String], target: &Path) -> Result<String, String> {
    let clips = files
        .iter()
        .map(|file| std::fs::read(file).map_err(|error| format!("Could not read {file}: {error}")))
        .collect::<Result<Vec<_>, _>>()?;
    let joined = webm::join(&clips)?;
    std::fs::write(target, joined).map_err(|error| format!("Could not save {}: {error}", target.display()))?;
    Ok(target.to_string_lossy().into_owned())
}

/// Makes one clip and writes it to the output folder. Returns the clip's file.
#[allow(clippy::too_many_arguments)]
async fn make_one(
    app: &AppHandle,
    queue: &Queue,
    engine: &Engine,
    generation: u64,
    job: &Job,
    layer: usize,
    clip: usize,
    picture: &mut Option<(usize, String)>,
) -> Result<String, String> {
    let layer_job = &job.layers[layer];
    /* The three clips of a layer start from the same picture, so it is made once. */
    if picture.as_ref().is_none_or(|(made_for, _)| *made_for != layer) {
        let (document, index, size) = (job.document.clone(), layer_job.index, (job.clip_width, job.clip_height));
        let data_url = tauri::async_runtime::spawn_blocking(move || {
            let layers = document::read_layers(Path::new(&document))?;
            if index >= layers.len() {
                return Err("The document changed and no longer has this layer.".to_string());
            }
            document::layer_start_picture(Path::new(&document), &layers, index, size.0, size.1)
        })
        .await
        .map_err(|error| format!("Reading the layer stopped unexpectedly: {error}"))??;
        *picture = Some((layer, data_url));
    }
    let image = picture.as_ref().map(|(_, data_url)| data_url.as_str()).unwrap_or_default();
    let files = settings::load(app).files;
    let report = |stage: &'static str, progress: f64| {
        update(app, queue, generation, false, |inner| {
            inner.current = Some(Current { layer, clip, stage, progress });
        });
    };
    let bytes = engine::make_clip(app, engine, &files, &layer_job.prompt, (job.clip_width, job.clip_height), image, |step| match step {
        Step::Loading => report("loading", 0.0),
        Step::Sampling { step, preview } => {
            report("generating", step as f64 / f64::from(VIDEO_STEPS));
            if let Some(preview) = preview {
                let _ = app.emit("clip-preview", format!("data:image/png;base64,{preview}"));
            }
        }
        Step::Finishing => report("finishing", 1.0),
    })
    .await?;
    let name = format!("{}-{:02}-{}-{}", document_stem(&job.document), layer_job.index + 1, slug(&layer_job.name), clip + 1);
    let path = unused_path(Path::new(&job.output_dir), &name);
    std::fs::write(&path, bytes).map_err(|error| format!("Could not save {}: {error}", path.display()))?;
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(statuses: &[[ClipStatus; 3]]) -> Job {
        Job {
            document: r"D:\Art\My Fox (final).imagesage".into(),
            output_dir: r"D:\Out".into(),
            combine: true,
            clip_width: 832,
            clip_height: 464,
            layers: statuses
                .iter()
                .enumerate()
                .map(|(index, clips)| LayerJob {
                    index,
                    tile: format!("history/{:04}-layer.png", index + 1),
                    name: format!("Layer {}", index + 1),
                    prompt: String::new(),
                    clips: clips
                        .iter()
                        .enumerate()
                        .map(|(clip, &status)| Clip { status, file: (status == ClipStatus::Done).then(|| format!("{index}-{clip}.webm")), error: None })
                        .collect(),
                })
                .collect(),
            combined: None,
        }
    }

    #[test]
    fn clips_are_made_bottom_layer_first() {
        use ClipStatus::*;
        let two_layers = job(&[[Done, Failed, Done], [Done, Pending, Pending]]);
        assert_eq!(two_layers.next_pending(), Some((1, 1)));
        assert_eq!((two_layers.count(Done), two_layers.count(Failed), two_layers.total()), (3, 1, 6));
        assert_eq!(two_layers.finished_files(), vec!["0-0.webm", "0-2.webm", "1-0.webm"]);
        assert_eq!(job(&[[Done, Done, Failed]]).next_pending(), None);
    }

    #[test]
    fn file_names_are_safe_and_never_replace_a_file() {
        assert_eq!(slug("add a red hat: with/feather?"), "add-a-red-hat-with-feather");
        assert_eq!(slug("///"), "layer");
        assert_eq!(slug(&"word ".repeat(30)).len(), 39);
        assert_eq!(document_stem(r"D:\Art\My Fox (final).imagesage"), "My-Fox-final");
        let dir = std::env::temp_dir().join(format!("imagesagevideo-names-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(unused_path(&dir, "fox-01-hat-1"), dir.join("fox-01-hat-1.webm"));
        std::fs::write(dir.join("fox-01-hat-1.webm"), b"x").unwrap();
        assert_eq!(unused_path(&dir, "fox-01-hat-1"), dir.join("fox-01-hat-1-2.webm"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_tray_text_shows_the_state() {
        use ClipStatus::*;
        let inner = Inner {
            job: Some(job(&[[Done, Running, Pending]])),
            state: RunState::Running,
            current: Some(Current { layer: 0, clip: 1, stage: "generating", progress: 0.35 }),
            error: None,
            task: None,
            generation: 1,
        };
        assert_eq!(tray_text(&inner.snapshot()), "Image Sage Video: 1 of 3 clips done, this clip 35%");
    }
}
