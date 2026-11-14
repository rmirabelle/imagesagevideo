use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::{
    crypto, document,
    engine::{self, Engine, Step, VIDEO_STEPS},
    files, last_frame, settings, tray, webm,
};

/**
 * The job: one video for each chosen layer. The layers can come from many
 * documents and photos, and more layers can be added while the job runs;
 * each layer keeps its own file and its own clip size. The user
 * chooses how many 5-second clips a video has. The first clip starts from
 * the layer's picture. Each clip after it starts from the last picture of
 * the clip before, with the same prompt, so the clips continue one another.
 * When a layer's clips are all done, they are joined into the layer's video
 * and the clips are deleted.
 *
 * The job runs in the Rust side of the app, so it goes on while the window
 * is hidden in the tray. The clips are made one after the other, in the
 * order of the job's layers; a document's bottom layer is first.
 *
 * Pause stops the clip that is in work and ends the engine, which removes
 * the model from the GPU memory. The engine cannot keep half a clip, so
 * Resume makes that clip again from its start.
 *
 * The job list is kept in an encrypted file after each change, so a job
 * that was not finished when the app closed can go on later. A clip is in
 * memory until it is finished; then it is written to the output folder as
 * a normal WebM file, and the next clip reads its last picture from there.
 */
pub const DEFAULT_CLIPS_PER_LAYER: usize = 2;
pub const MAX_CLIPS_PER_LAYER: usize = 20;
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
    /// The user removed this finished clip from the list. It stays in the layer's video.
    #[serde(default)]
    cleared: bool,
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
    /// The layer's video: its clips, joined. The clips' own files are deleted when it is made.
    #[serde(default)]
    video: Option<String>,
    /// The layer's document or photo.
    #[serde(default)]
    document: String,
    /// The size of this layer's clips: the shape of its document.
    #[serde(default)]
    clip_width: u32,
    #[serde(default)]
    clip_height: u32,
}

impl LayerJob {
    /// The start of the layer's file names. A photo has one layer, so the photo's own name is enough.
    fn file_stem(&self) -> String {
        if self.tile == document::PHOTO_TILE {
            document_stem(&self.document)
        } else {
            format!("{}-{:02}-{}", document_stem(&self.document), self.index + 1, slug(&self.name))
        }
    }

    fn all_done(&self) -> bool {
        self.clips.iter().all(|clip| clip.status == ClipStatus::Done)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    output_dir: String,
    layers: Vec<LayerJob>,
    /// A job list from before version 0.6 had one document for all its layers. `restore` moves these values to the layers.
    #[serde(default, skip_serializing)]
    document: String,
    #[serde(default, skip_serializing)]
    clip_width: u32,
    #[serde(default, skip_serializing)]
    clip_height: u32,
}

impl Job {
    /// Gives each layer of an old job list the job's one document and clip size.
    fn fill_layers_from_old_job(&mut self) {
        for layer in self.layers.iter_mut().filter(|layer| layer.document.is_empty()) {
            layer.document = self.document.clone();
            layer.clip_width = self.clip_width;
            layer.clip_height = self.clip_height;
        }
    }

    fn next_pending(&self) -> Option<(usize, usize)> {
        self.layers.iter().enumerate().find_map(|(layer, job)| {
            job.clips.iter().position(|clip| clip.status == ClipStatus::Pending).map(|clip| (layer, clip))
        })
    }

    /// The clips that are in the list: those the user did not remove.
    fn listed(&self) -> impl Iterator<Item = &Clip> {
        self.layers.iter().flat_map(|layer| &layer.clips).filter(|clip| !clip.cleared)
    }

    fn count(&self, status: ClipStatus) -> usize {
        self.listed().filter(|clip| clip.status == status).count()
    }

    fn total(&self) -> usize {
        self.listed().count()
    }

    /**
     * Removes the clip that is in work from its layer, so the layer's video
     * has one clip less. The clips after it move one place down; the next
     * one then continues the last done clip. Returns false when no clip is
     * in work at that place.
     */
    fn skip_running(&mut self, layer: usize, clip: usize) -> bool {
        let Some(clips) = self.layers.get_mut(layer).map(|layer| &mut layer.clips) else { return false };
        if clips.get(clip).is_none_or(|clip| clip.status != ClipStatus::Running) {
            return false;
        }
        clips.remove(clip);
        true
    }

    /**
     * Removes each done clip from the list. The clip stays in the job, so
     * the places of the other clips do not change and the layer's video
     * still has it. Returns how many clips it removed.
     */
    fn clear_done(&mut self) -> usize {
        let mut removed = 0;
        for clip in self.layers.iter_mut().flat_map(|layer| &mut layer.clips) {
            if clip.status == ClipStatus::Done && !clip.cleared {
                clip.cleared = true;
                removed += 1;
            }
        }
        removed
    }

    /// True when the file is a clip that is not joined into a video yet.
    fn has_clip_file(&self, path: &str) -> bool {
        self.layers.iter().flat_map(|layer| &layer.clips).any(|clip| clip.file.as_deref().is_some_and(|file| files::same_file(file, path)))
    }

    /// The layer that still needs this document or photo for its first clip.
    fn layer_that_needs(&self, path: &str) -> Option<&LayerJob> {
        self.layers.iter().find(|layer| {
            files::same_file(&layer.document, path) && layer.video.is_none() && layer.clips.first().is_some_and(|clip| clip.status != ClipStatus::Done)
        })
    }

    /**
     * Gives the job the new path of a file that was renamed: a layer's
     * document or photo, or a layer's video. A photo's layer has the
     * photo's name, so it gets the new name. Returns false when the job
     * does not use the file.
     */
    fn repath(&mut self, old: &str, new: &str) -> bool {
        let stem = |path: &str| Path::new(path).file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
        let mut changed = false;
        for layer in &mut self.layers {
            if files::same_file(&layer.document, old) {
                layer.document = new.to_string();
                if layer.tile == document::PHOTO_TILE && layer.name == stem(old) {
                    layer.name = stem(new);
                }
                changed = true;
            }
            if layer.video.as_deref().is_some_and(|video| files::same_file(video, old)) {
                layer.video = Some(new.to_string());
                changed = true;
            }
        }
        changed
    }

    /// Removes from the list the layer whose video is this file. Returns false when no layer has this video.
    fn forget_video(&mut self, path: &str) -> bool {
        let mut changed = false;
        for layer in self.layers.iter_mut().filter(|layer| layer.video.as_deref().is_some_and(|video| files::same_file(video, path))) {
            for clip in &mut layer.clips {
                clip.cleared = true;
            }
            changed = true;
        }
        changed
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
    /// Why the job paused by itself, or why a layer's video could not be made.
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

    /// Removes a job that does not run and has no clip left in the list.
    fn drop_empty_job(&mut self) {
        if self.state != RunState::Running && self.job.as_ref().is_some_and(|job| job.listed().next().is_none()) {
            self.generation += 1;
            self.job = None;
            self.state = RunState::Idle;
            self.current = None;
            self.error = None;
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
        job.fill_layers_from_old_job();
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
pub struct StartLayer {
    /// The layer's document or photo.
    document: String,
    /// The layer's file in the document.
    tile: String,
    /// The prompt the user wrote in place of the document's prompt. The document itself does not change.
    prompt: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRequest {
    /// The folder for a new job's videos. A job that exists keeps its folder.
    output_dir: String,
    /// How many 5-second clips each layer's video has.
    clips: usize,
    /// The layers to make a video of.
    layers: Vec<StartLayer>,
}

#[tauri::command]
pub fn queue_get(queue: State<'_, Queue>) -> Snapshot {
    queue.lock().snapshot()
}

/// Reads the documents of a start request and makes the job's layers: the documents in the request's order, each with its bottom layer first.
fn layer_jobs(request: &[StartLayer], clips: usize) -> Result<Vec<LayerJob>, String> {
    let mut documents: Vec<&str> = Vec::new();
    for layer in request {
        if !documents.contains(&layer.document.as_str()) {
            documents.push(&layer.document);
        }
    }
    let mut jobs = Vec::new();
    for document in documents {
        let path = Path::new(document);
        let layers = document::read_layers(path)?;
        let (clip_width, clip_height) = document::document_clip_size(path, &layers)?;
        for (index, layer) in layers.into_iter().enumerate() {
            let Some(asked) = request.iter().find(|asked| asked.document == document && asked.tile == layer.tile) else { continue };
            jobs.push(LayerJob {
                index,
                prompt: asked.prompt.as_ref().map(|prompt| prompt.trim().to_string()).unwrap_or(layer.prompt),
                tile: layer.tile,
                name: layer.name,
                clips: vec![Clip { status: ClipStatus::Pending, file: None, error: None, cleared: false }; clips],
                video: None,
                document: crate::files::windows_path(document),
                clip_width,
                clip_height,
            });
        }
    }
    Ok(jobs)
}

/**
 * Starts a job, or adds layers to the job that exists. A job that runs
 * goes on and makes the new layers after its other layers. A paused job
 * stays paused. A finished job starts again.
 */
#[tauri::command]
pub async fn queue_start(app: AppHandle, queue: State<'_, Queue>, engine: State<'_, Engine>, request: StartRequest) -> Result<(), String> {
    let missing = settings::load(&app).files.missing();
    if !missing.is_empty() {
        return Err(format!("Choose the Wan 2.2 {} in Settings.", missing.join(", ")));
    }
    let clips = request.clips.clamp(1, MAX_CLIPS_PER_LAYER);
    let asked = request.layers;
    let jobs = tauri::async_runtime::spawn_blocking(move || layer_jobs(&asked, clips))
        .await
        .map_err(|error| format!("Reading the document stopped unexpectedly: {error}"))??;
    if jobs.is_empty() {
        return Err("Choose one layer or more first.".into());
    }
    let mut saved = settings::load(&app);
    saved.clips_per_layer = clips;
    let mut inner = queue.lock();
    if let Some(job) = inner.job.as_mut() {
        job.layers.extend(jobs);
        settings::save(&app, &saved)?;
        if inner.state == RunState::Finished {
            spawn_runner(&app, &queue, &engine, &mut inner);
        } else {
            save_job(&app, inner.job.as_ref());
            publish(&app, &inner);
        }
        return Ok(());
    }
    let output_dir = crate::files::windows_path(&request.output_dir);
    let output = PathBuf::from(&output_dir);
    if output_dir.is_empty() || !output.is_absolute() {
        return Err("Choose the folder for the videos first.".into());
    }
    std::fs::create_dir_all(&output).map_err(|error| format!("Could not use the folder {}: {error}", output.display()))?;
    let job = Job { output_dir: output.to_string_lossy().into_owned(), layers: jobs, document: String::new(), clip_width: 0, clip_height: 0 };
    saved.output_dir = job.output_dir.clone();
    settings::save(&app, &saved)?;
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
            *clip = Clip { status: ClipStatus::Pending, file: None, error: None, cleared: false };
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

/**
 * Removes the done clips from the list; the job can be running. The
 * videos stay in their folder. The runner goes on: the clips keep their
 * places in the job, so the generation does not change. When no clip is
 * left in a job that does not run, the job itself is removed.
 */
#[tauri::command]
pub fn queue_clear_done(app: AppHandle, queue: State<'_, Queue>) {
    let mut inner = queue.lock();
    let Some(job) = inner.job.as_mut() else { return };
    if job.clear_done() == 0 {
        return;
    }
    inner.drop_empty_job();
    save_job(&app, inner.job.as_ref());
    publish(&app, &inner);
}

/**
 * Stops the clip in work and removes it from its layer; the job then goes
 * on with the next clip. The engine stays loaded: when the stopped runner
 * is dropped, it tells the engine to cancel that clip. A new runner starts,
 * so the stopped one cannot write stale state. When no clip is left in the
 * list, the job itself is removed.
 */
#[tauri::command]
pub fn queue_cancel_clip(app: AppHandle, queue: State<'_, Queue>, engine: State<'_, Engine>) -> Result<(), String> {
    let mut inner = queue.lock();
    let place = inner.current.as_ref().filter(|current| current.stage != "joining").map(|current| (current.layer, current.clip));
    let skipped = inner.state == RunState::Running
        && place.is_some_and(|(layer, clip)| inner.job.as_mut().is_some_and(|job| job.skip_running(layer, clip)));
    if !skipped {
        return Err("No clip is in work now.".into());
    }
    inner.generation += 1;
    if let Some(task) = inner.task.take() {
        task.abort();
    }
    if inner.job.as_ref().is_some_and(|job| job.listed().next().is_none()) {
        inner.job = None;
        inner.state = RunState::Idle;
        inner.current = None;
        inner.error = None;
        save_job(&app, None);
        publish(&app, &inner);
        drop(inner);
        engine.stop();
        return Ok(());
    }
    spawn_runner(&app, &queue, &engine, &mut inner);
    Ok(())
}

/**
 * Changes a layer's prompt; the job can be running. The clip in work keeps
 * the prompt it started with. Each clip of the layer that starts after
 * this uses the new prompt. The runner goes on, so the generation does not
 * change.
 */
#[tauri::command]
pub fn queue_set_prompt(app: AppHandle, queue: State<'_, Queue>, layer: usize, prompt: String) -> Result<(), String> {
    let mut inner = queue.lock();
    let layer = inner.job.as_mut().and_then(|job| job.layers.get_mut(layer)).ok_or("The job does not have this layer.")?;
    if layer.video.is_some() {
        return Err("This layer's video is done, so its prompt cannot change.".into());
    }
    layer.prompt = prompt.trim().to_string();
    save_job(&app, inner.job.as_ref());
    publish(&app, &inner);
    Ok(())
}

/**
 * Renames a document, a photo or a finished video on the disk; the file
 * stays in its folder and keeps its extension. A job that uses the file
 * gets the new path, and the runner goes on, so the generation does not
 * change. Returns the new path.
 */
#[tauri::command]
pub fn file_rename(app: AppHandle, queue: State<'_, Queue>, path: String, name: String) -> Result<String, String> {
    let old = files::managed_file(&path)?;
    let new = files::renamed_path(&old, &name)?;
    let (old_text, new_text) = (old.to_string_lossy().into_owned(), new.to_string_lossy().into_owned());
    if new == old {
        return Ok(old_text);
    }
    if new.exists() && !files::same_file(&old_text, &new_text) {
        return Err(format!("{} exists already. Write a different name.", new.display()));
    }
    let mut inner = queue.lock();
    let running = inner.state == RunState::Running;
    if let Some(job) = inner.job.as_ref() {
        if job.has_clip_file(&old_text) {
            return Err("The job still uses this clip. Wait until the clips are joined.".into());
        }
        /* The runner has the old path while it makes a layer's first clip. */
        let read_now = job.layers.iter().any(|layer| {
            files::same_file(&layer.document, &old_text) && layer.clips.first().is_some_and(|clip| clip.status == ClipStatus::Running)
        });
        if running && read_now {
            return Err("The job makes the first clip from this file now. Rename the file when that clip is done.".into());
        }
    }
    std::fs::rename(&old, &new).map_err(|error| format!("Could not rename {}: {error}", old.display()))?;
    if inner.job.as_mut().is_some_and(|job| job.repath(&old_text, &new_text)) {
        save_job(&app, inner.job.as_ref());
        publish(&app, &inner);
    }
    Ok(new_text)
}

/**
 * Moves a document, a photo or a finished video to the Recycle Bin. A
 * file that the job still needs for a layer's first clip stays. A deleted
 * video's layer leaves the list; when no clip is left in a job that does
 * not run, the job itself is removed.
 */
#[tauri::command]
pub fn file_delete(app: AppHandle, queue: State<'_, Queue>, path: String) -> Result<(), String> {
    let file = files::managed_file(&path)?;
    let text = file.to_string_lossy().into_owned();
    let mut inner = queue.lock();
    if let Some(job) = inner.job.as_ref() {
        if job.has_clip_file(&text) {
            return Err("The job still uses this clip. Wait until the clips are joined.".into());
        }
        if let Some(layer) = job.layer_that_needs(&text) {
            return Err(format!("The job needs this file for the first clip of \"{}\". Delete the file when that clip is done, or remove the job first.", layer.name));
        }
    }
    files::recycle(&file)?;
    if inner.job.as_mut().is_some_and(|job| job.forget_video(&text)) {
        inner.drop_empty_job();
        save_job(&app, inner.job.as_ref());
        publish(&app, &inner);
    }
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

/**
 * Makes the job's clips, one after the other. When a layer's clips are all
 * done, it joins them into the layer's video. The user can add layers while
 * the job runs, so the runner looks for a new clip again before it ends.
 */
async fn run(app: AppHandle, queue: Queue, engine: Engine, generation: u64) {
    let mut picture: Option<(usize, String)> = None;
    let mut failures_in_a_row = 0_u32;
    loop {
        /* A layer can have all its clips done but not joined: the user cancelled its last clip, or the app closed before the join. */
        let Some(layers) = update(&app, &queue, generation, false, |inner| inner.job.as_ref().map_or(0, |job| job.layers.len())) else { return };
        for layer in 0..layers {
            if join_layer(&app, &queue, generation, layer).await.is_none() {
                return;
            }
        }
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
                let clips = &mut job.layers[layer].clips;
                match &made {
                    Ok(file) => clips[clip] = Clip { status: ClipStatus::Done, file: Some(file.clone()), error: None, cleared: false },
                    Err(error) => {
                        clips[clip] = Clip { status: ClipStatus::Failed, file: None, error: Some(error.clone()), cleared: false };
                        /* The clips after it have no picture to start from. Retry makes them again, in order. */
                        for later in clips[clip + 1..].iter_mut().filter(|later| later.status == ClipStatus::Pending) {
                            later.status = ClipStatus::Failed;
                            later.error = Some("The clip before this one failed, so this clip was not made.".into());
                        }
                    }
                }
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
            if made.is_ok() && join_layer(&app, &queue, generation, layer).await.is_none() {
                return;
            }
        }
        /* Every clip is done or failed: the engine ends, so the model leaves the GPU memory. This is before the job becomes finished, so a runner that starts after this one cannot lose its engine. */
        engine.stop();
        let finished = update(&app, &queue, generation, true, |inner| {
            if inner.job.as_ref().is_some_and(|job| job.next_pending().is_some()) {
                return false;
            }
            inner.state = RunState::Finished;
            inner.current = None;
            inner.task = None;
            true
        });
        match finished {
            None | Some(true) => return,
            Some(false) => {}
        }
    }
}

/**
 * When the layer's clips are all done, joins them into the layer's video
 * and deletes the clips. Returns None when this runner was paused or
 * replaced; it must then stop.
 *
 * The join and the change to the job run in one piece that a pause cannot
 * cut in two. If the job was paused during the join, the new video is
 * removed and the clips stay, so Resume joins them again.
 */
async fn join_layer(app: &AppHandle, queue: &Queue, generation: u64, layer: usize) -> Option<()> {
    let work = update(app, queue, generation, false, |inner| {
        let job = inner.job.as_ref()?;
        let layer_job = job.layers.get(layer)?;
        if layer_job.video.is_some() || layer_job.clips.is_empty() || !layer_job.all_done() {
            return None;
        }
        let files: Vec<String> = layer_job.clips.iter().filter_map(|clip| clip.file.clone()).collect();
        if files.len() != layer_job.clips.len() {
            return None;
        }
        let target = unused_path(Path::new(&job.output_dir), &layer_job.file_stem());
        inner.current = Some(Current { layer, clip: 0, stage: "joining", progress: 0.0 });
        Some((files, target, layer_job.name.clone()))
    })?;
    let Some((files, target, name)) = work else { return Some(()) };
    let (app, queue) = (app.clone(), queue.clone());
    tauri::async_runtime::spawn_blocking(move || {
        let joined = join_files(&files, &target);
        let kept = update(&app, &queue, generation, true, |inner| {
            inner.current = None;
            match &joined {
                Ok(file) => {
                    if let Some(layer_job) = inner.job.as_mut().and_then(|job| job.layers.get_mut(layer)) {
                        layer_job.video = Some(file.clone());
                        for clip in &mut layer_job.clips {
                            clip.file = None;
                        }
                    }
                }
                Err(error) => inner.error = Some(format!("The video of the layer \"{name}\" was not made. {error}")),
            }
        });
        match (kept, joined) {
            (Some(()), Ok(_)) => {
                for file in &files {
                    let _ = std::fs::remove_file(file);
                }
            }
            (None, Ok(file)) => {
                let _ = std::fs::remove_file(file);
            }
            _ => {}
        }
        kept
    })
    .await
    .ok()
    .flatten()
}

/// Reads a layer's clips and writes them, joined, to `target`. Each clip continues the clip before it.
fn join_files(files: &[String], target: &Path) -> Result<String, String> {
    let clips = files
        .iter()
        .map(|file| std::fs::read(file).map_err(|error| format!("Could not read {file}: {error}")))
        .collect::<Result<Vec<_>, _>>()?;
    let joined = webm::join(&clips, true)?;
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
    let size = (layer_job.clip_width, layer_job.clip_height);
    let continued;
    let image = if clip == 0 {
        /* The first clip starts from the layer's picture. It is kept, so Retry does not make it again. */
        if picture.as_ref().is_none_or(|(made_for, _)| *made_for != layer) {
            let (document, index) = (layer_job.document.clone(), layer_job.index);
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
        picture.as_ref().map(|(_, data_url)| data_url.as_str()).unwrap_or_default()
    } else {
        /* Each clip after the first starts from the last picture of the clip before it. */
        let before = layer_job.clips[clip - 1].file.clone().ok_or("The clip before this one has no file, so this clip cannot continue it.")?;
        continued = tauri::async_runtime::spawn_blocking(move || {
            let bytes = std::fs::read(&before).map_err(|error| format!("Could not read {before}: {error}"))?;
            last_frame::start_picture(&bytes, size.0, size.1)
        })
        .await
        .map_err(|error| format!("Reading the clip before this one stopped unexpectedly: {error}"))??;
        continued.as_str()
    };
    let files = settings::load(app).files;
    let report = |stage: &'static str, progress: f64| {
        update(app, queue, generation, false, |inner| {
            inner.current = Some(Current { layer, clip, stage, progress });
        });
    };
    let bytes = engine::make_clip(app, engine, &files, &layer_job.prompt, size, image, |step| match step {
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
    let name = format!("{}-{}", layer_job.file_stem(), clip + 1);
    let path = unused_path(Path::new(&job.output_dir), &name);
    std::fs::write(&path, bytes).map_err(|error| format!("Could not save {}: {error}", path.display()))?;
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(statuses: &[[ClipStatus; 3]]) -> Job {
        Job {
            document: String::new(),
            output_dir: r"D:\Out".into(),
            clip_width: 0,
            clip_height: 0,
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
                        .map(|(clip, &status)| Clip { status, file: (status == ClipStatus::Done).then(|| format!("{index}-{clip}.webm")), error: None, cleared: false })
                        .collect(),
                    video: None,
                    document: r"D:\Art\My Fox (final).imagesage".into(),
                    clip_width: 832,
                    clip_height: 464,
                })
                .collect(),
        }
    }

    #[test]
    fn an_old_job_list_gives_its_document_to_each_layer() {
        let old = serde_json::json!({
            "document": "D:/a.imagesage", "outputDir": "D:/Out", "clipWidth": 832, "clipHeight": 464,
            "layers": [{ "index": 0, "tile": "history/0001-layer.png", "name": "Fox", "prompt": "", "clips": [] }]
        });
        let mut job: Job = serde_json::from_value(old).unwrap();
        job.fill_layers_from_old_job();
        assert_eq!((job.layers[0].document.as_str(), job.layers[0].clip_width, job.layers[0].clip_height), ("D:/a.imagesage", 832, 464));
        let saved = serde_json::to_value(&job).unwrap();
        assert!(saved.get("document").is_none());
        assert_eq!(saved["layers"][0]["clipWidth"], 832);
    }

    #[test]
    fn a_photo_has_a_short_file_name() {
        let mut job = job(&[[ClipStatus::Pending; 3]]);
        assert_eq!(job.layers[0].file_stem(), "My-Fox-final-01-Layer-1");
        job.layers[0].tile = document::PHOTO_TILE.into();
        job.layers[0].document = r"D:\Photos\Beach day.jpg".into();
        assert_eq!(job.layers[0].file_stem(), "Beach-day");
    }

    #[test]
    fn a_renamed_or_deleted_file_changes_the_job() {
        use ClipStatus::*;
        let mut job = job(&[[Done, Done, Done], [Pending, Pending, Pending]]);
        job.layers[0].video = Some(r"D:\Out\fox.webm".into());
        for clip in &mut job.layers[0].clips {
            clip.file = None;
        }
        assert!(!job.repath(r"D:\Other\file.png", r"D:\Other\new.png"));
        assert!(job.repath("d:/art/my fox (final).imagesage", r"D:\Art\Fox.imagesage"));
        assert!(job.layers.iter().all(|layer| layer.document == r"D:\Art\Fox.imagesage"));
        assert_eq!(job.layer_that_needs(r"D:\Art\Fox.imagesage").map(|layer| layer.index), Some(1));
        assert!(job.repath(r"D:\Out\fox.webm", r"D:\Out\red fox.webm"));
        assert_eq!(job.layers[0].video.as_deref(), Some(r"D:\Out\red fox.webm"));
        assert!(!job.has_clip_file(r"D:\Out\red fox.webm"));
        assert!(job.forget_video(r"d:\out\RED FOX.webm"));
        assert_eq!(job.total(), 3);
        job.layers[1].tile = document::PHOTO_TILE.into();
        job.layers[1].name = "Fox".into();
        assert!(job.repath(r"D:\Art\Fox.imagesage", r"D:\Art\Wolf.imagesage"));
        assert_eq!(job.layers[1].name, "Wolf");
    }

    #[test]
    fn a_cancelled_clip_leaves_its_layer() {
        let mut job = job(&[[ClipStatus::Done, ClipStatus::Running, ClipStatus::Pending]]);
        assert!(!job.skip_running(0, 0));
        assert!(!job.skip_running(1, 0));
        assert!(job.skip_running(0, 1));
        assert_eq!(job.layers[0].clips.len(), 2);
        assert_eq!(job.next_pending(), Some((0, 1)));
    }

    #[test]
    fn clearing_removes_only_the_done_clips_from_the_list() {
        use ClipStatus::*;
        let mut job = job(&[[Done, Done, Done], [Done, Failed, Done], [Done, Running, Pending]]);
        assert_eq!(job.clear_done(), 6);
        assert_eq!(job.clear_done(), 0);
        assert_eq!((job.count(Done), job.count(Failed), job.total()), (0, 1, 3));
        /* The runner's places do not change, and the clips keep their files for the layer's video. */
        assert_eq!(job.next_pending(), Some((2, 2)));
        assert_eq!(job.layers.iter().flat_map(|layer| &layer.clips).filter(|clip| clip.file.is_some()).count(), 6);
        assert!(job.layers[0].all_done() && !job.layers[1].all_done());
    }

    #[test]
    fn clips_are_made_bottom_layer_first() {
        use ClipStatus::*;
        let two_layers = job(&[[Done, Failed, Done], [Done, Pending, Pending]]);
        assert_eq!(two_layers.next_pending(), Some((1, 1)));
        assert_eq!((two_layers.count(Done), two_layers.count(Failed), two_layers.total()), (3, 1, 6));
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
