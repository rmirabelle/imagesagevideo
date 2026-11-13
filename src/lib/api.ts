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
}

export interface Job {
  document: string;
  outputDir: string;
  clipWidth: number;
  clipHeight: number;
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
export const queueStart = (request: { document: string; outputDir: string; clips: number; tiles: string[]; prompts: Record<string, string> }) =>
  invoke<void>("queue_start", { request });
export const queuePause = () => invoke<void>("queue_pause");
export const queueResume = () => invoke<void>("queue_resume");
export const queueClear = () => invoke<void>("queue_clear");
export const queueClearDone = () => invoke<void>("queue_clear_done");
export const settingsGet =() => invoke<SettingsView>("settings_get");
export const settingsSet = (settings: Settings) => invoke<void>("settings_set", { settings });
export const settingsFromImageSage = () => invoke<ModelFiles>("settings_from_image_sage");
export const engineStatus = () => invoke<EngineStatus>("engine_status");
export const engineDownload = () => invoke<void>("engine_download");
export const findModels = (folder: string) => invoke<FoundModels>("find_models", { folder });
export const revealPath = (path: string) => invoke<void>("reveal_path", { path });
export const playVideo = (path: string) => invoke<void>("play_video", { path });
export const hideToTray = () => invoke<void>("hide_to_tray");
export const quitApp = () => invoke<void>("quit_app");
export const takePendingOpenDocument = () => invoke<string | null>("take_pending_open_document");

/** The last part of a path: the file's or the folder's name. */
export const baseName = (path: string) => path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? path;

/** The folder that holds a file. */
export const folderOf = (path: string) => path.replace(/[\\/][^\\/]*$/, "");
