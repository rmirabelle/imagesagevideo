import { invoke } from "@tauri-apps/api/core";

/** One layer of an opened document, bottom layer first. */
export interface LayerInfo {
  /** The layer's file in the document; it identifies the layer. */
  tile: string;
  name: string;
  prompt: string;
  /** The layer is turned off in Image Sage. */
  hidden: boolean;
  /** A small JPEG of the layer, as a data URL. */
  thumbnail: string;
}

export interface DocumentInfo {
  path: string;
  width: number;
  height: number;
  /** The size of each clip; null when the document is wider or taller than 3:1. */
  clipWidth: number | null;
  clipHeight: number | null;
  layers: LayerInfo[];
}

export type ClipStatus = "pending" | "running" | "done" | "failed";

export interface Clip {
  status: ClipStatus;
  file: string | null;
  error: string | null;
  /** The user removed this finished clip from the list. */
  cleared: boolean;
}

export interface LayerJob {
  index: number;
  tile: string;
  name: string;
  prompt: string;
  clips: Clip[];
  /** The layer's video: its clips, joined. The clips have no files of their own then. */
  video: string | null;
  /** The layer's document or photo. */
  document: string;
  /** The size of this layer's clips. */
  clipWidth: number;
  clipHeight: number;
}

/** The job's layers can come from many documents and photos. */
export interface Job {
  outputDir: string;
  layers: LayerJob[];
}

export type RunState = "idle" | "running" | "paused" | "finished";

export interface Current {
  layer: number;
  clip: number;
  stage: "preparing" | "loading" | "generating" | "finishing" | "joining";
  progress: number;
}

export interface Snapshot {
  job: Job | null;
  state: RunState;
  current: Current | null;
  error: string | null;
  clipsDone: number;
  clipsFailed: number;
  clipsTotal: number;
}

export interface ModelFiles {
  diffusionModel: string;
  highNoiseModel: string;
  textEncoder: string;
  vae: string;
  negativePrompt: string;
  /** The LoRAs of every clip, in the sequence the engine gets them. */
  loras: LoraPair[];
}

/** One LoRA for the two Wan 2.2 models: a file for each model, with its strength. An empty file means none for that model. */
export interface LoraPair {
  highNoise: string;
  highNoiseStrength: number;
  lowNoise: string;
  lowNoiseStrength: number;
}

export interface Settings {
  files: ModelFiles;
  outputDir: string;
  /** How many 5-second clips each layer's video had in the last job. */
  clipsPerLayer: number;
  /** The folder the last LoRA file was chosen from. */
  loraFolder: string;
}

export interface SettingsView {
  settings: Settings;
  /** The model files that are not set or not found, by name. */
  missing: string[];
  imageSageAvailable: boolean;
  defaultNegativePrompt: string;
}

export interface EngineStatus {
  installed: boolean;
  sharedWithImageSage: boolean;
  sizeBytes: number;
  loaded: boolean;
}

export interface FoundModels {
  diffusionModel: string | null;
  highNoiseModel: string | null;
  textEncoder: string | null;
  vae: string | null;
}

export const openDocument = (path: string) => invoke<DocumentInfo>("open_document", { path });
/** One layer's own picture, large, as a JPEG data URL. */
export const layerPicture = (path: string, tile: string) => invoke<string>("layer_picture", { path, tile });
export const queueGet =() => invoke<Snapshot>("queue_get");
/** Starts a job, or adds layers to the job that exists. A null prompt means the file's own prompt. */
export const queueStart = (request: { outputDir: string; clips: number; layers: { document: string; tile: string; prompt: string | null }[] }) =>
  invoke<void>("queue_start", { request });
export const queuePause = () => invoke<void>("queue_pause");
export const queueResume = () => invoke<void>("queue_resume");
export const queueClear = () => invoke<void>("queue_clear");
export const queueClearDone = () => invoke<void>("queue_clear_done");
/** Changes the prompt of a job's layer, by the layer's place in the job. The clips of that layer that start after this use it. */
export const queueSetPrompt = (layer: number, prompt: string) => invoke<void>("queue_set_prompt", { layer, prompt });
/** Stops the clip in work and removes it from its layer; the job goes on with the next clip. */
export const queueCancelClip = () => invoke<void>("queue_cancel_clip");
export const settingsGet =() => invoke<SettingsView>("settings_get");
export const settingsSet = (settings: Settings) => invoke<void>("settings_set", { settings });
export const settingsFromImageSage = () => invoke<ModelFiles>("settings_from_image_sage");
export const engineStatus = () => invoke<EngineStatus>("engine_status");
export const engineDownload = () => invoke<void>("engine_download");
export const findModels = (folder: string) => invoke<FoundModels>("find_models", { folder });
export const revealPath = (path: string) => invoke<void>("reveal_path", { path });
export const playVideo = (path: string) => invoke<void>("play_video", { path });
/** Renames a document, a photo or a finished video on the disk. The name has no extension; the file keeps its own. Returns the new path. */
export const fileRename = (path: string, name: string) => invoke<string>("file_rename", { path, name });
/** Moves a document, a photo or a finished video to the Recycle Bin. */
export const fileDelete = (path: string) => invoke<void>("file_delete", { path });
export const hideToTray = () => invoke<void>("hide_to_tray");
export const quitApp = () => invoke<void>("quit_app");
export const takePendingOpenDocument = () => invoke<string | null>("take_pending_open_document");

/** The last part of a path: the file's or the folder's name. */
export const baseName = (path: string) => path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? path;

/** The folder that holds a file. */
export const folderOf = (path: string) => path.replace(/[\\/][^\\/]*$/, "");
