import { isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import {
  CheckCircle,
  CircleNotch,
  Copy,
  EyeSlash,
  File as FileIcon,
  FilmStrip,
  FolderOpen,
  Pause,
  PencilSimple,
  Play,
  Plus,
  Trash,
  WarningCircle,
  X
} from "@phosphor-icons/react";
import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { AboutDialog } from "./components/AboutDialog";
import { SettingsDialog } from "./components/SettingsDialog";
import { TitleBar } from "./components/TitleBar";
import {
  baseName,
  fileDelete,
  fileRename,
  hideToTray,
  layerPicture,
  openDocument,
  playVideo,
  queueClear,
  queueCancelClip,
  queueClearDone,
  queueSetPrompt,
  queueGet,
  queuePause,
  queueResume,
  queueStart,
  quitApp,
  revealPath,
  settingsGet,
  takePendingOpenDocument,
  type Clip,
  type Current,
  type DocumentInfo,
  type Snapshot
} from "./lib/api";
import { checkForUpdate, getAppVersion, type UpdateInfo } from "./lib/updater";

const CLIP_SECONDS = 5;
const MAX_CLIPS_PER_LAYER = 20;
/** The file name the Rust side gives to the one layer of a photo. */
const PHOTO_TILE = "photo";
const EMPTY: Snapshot = { job: null, state: "idle", current: null, error: null, clipsDone: 0, clipsFailed: 0, clipsTotal: 0 };

const STAGE_TEXT: Record<Current["stage"], string> = {
  preparing: "Preparing the layer's picture",
  loading: "Loading the model",
  generating: "Making the clip",
  finishing: "Finishing the clip",
  joining: "Joining the clips"
};

/** One name for a file, however its path is written. */
const fileKey = (path: string) => path.replace(/\//g, "\\").toLowerCase();
/** One name for a layer of a file. */
const layerKey = (path: string, tile: string) => `${fileKey(path)}|${tile}`;

/** One row of the list: a layer that waits for Start, or a layer of the job. */
interface Row {
  /** The row's name in the list: `s:` and the layer's key for a waiting layer, `j:` and the layer's place for a job layer. */
  id: string;
  key: string;
  path: string;
  tile: string;
  /** The layer's place in the job; -1 for a layer that waits for Start. */
  jobLayer: number;
  number: number;
  name: string;
  prompt: string;
  clips: Clip[] | null;
  video: string | null;
}

/** The rows as the list shows them: the files in their order, and the top layer of each file first, as Image Sage does. */
function topLayerFirst(rows: Row[]): Row[] {
  const shown: Row[] = [];
  let group: Row[] = [];
  for (const row of rows) {
    if (group.length && fileKey(group[0].path) !== fileKey(row.path)) {
      shown.push(...group.reverse());
      group = [];
    }
    group.push(row);
  }
  return [...shown, ...group.reverse()];
}

/**
 * The one window of Image Sage Video. The user opens documents and photos;
 * their layers wait in the list with a checkbox for each. Start makes a job
 * of the chosen layers. While a job exists, the list also shows the job's
 * layers with their clips and videos, and the user can open more files and
 * add their layers to the job. The job itself runs in the Rust side, so it
 * goes on while this window is hidden in the tray.
 */
export default function App() {
  const [version, setVersion] = useState("");
  const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null);
  const [aboutOpen, setAboutOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [closeAsked, setCloseAsked] = useState(false);

  const [snapshot, setSnapshot] = useState<Snapshot>(EMPTY);
  /* Each file the window read, by its file key. A job's files stay here for their small pictures. */
  const [known, setKnown] = useState<Map<string, DocumentInfo>>(new Map());
  /* The file keys of the files whose layers wait for Start, in the order the user opened them. */
  const [staged, setStaged] = useState<string[]>([]);
  const [opening, setOpening] = useState(false);
  const [chosen, setChosen] = useState<Set<string>>(new Set());
  /* The layers the user removed from the list. The file does not change, and they come back when it is opened again. */
  const [removed, setRemoved] = useState<Set<string>>(new Set());
  /* The prompts the user changed, by layer key. The file does not change; the job gets these prompts. */
  const [prompts, setPrompts] = useState<Map<string, string>>(new Map());
  /* The row whose prompt is in the text box now. */
  const [editing, setEditing] = useState<string | null>(null);
  const editCancelled = useRef(false);
  /* The row whose large picture shows. The small picture shows first, until the large one is read. */
  const [zoom, setZoom] = useState<{ id: string; name: string; src: string } | null>(null);
  /* The row whose prompt went to the clipboard a moment ago. Its button shows a check mark for a short time. */
  const [copied, setCopied] = useState<string | null>(null);
  /* The file whose name is in the text box now, by its file key. */
  const [renaming, setRenaming] = useState<string | null>(null);
  /* The file that the user asked to delete. The dialog asks before the file goes. */
  const [deleting, setDeleting] = useState<{ path: string; video: boolean } | null>(null);
  const [outputDir, setOutputDir] = useState("");
  const [clipsPerLayer, setClipsPerLayer] = useState(2);
  const [missing, setMissing] = useState<string[]>([]);
  const [message, setMessage] = useState<string | null>(null);
  const [preview, setPreview] = useState<string | null>(null);

  const job = snapshot.job;
  const running = snapshot.state === "running";

  const refreshSettings = useCallback(async () => {
    const view = await settingsGet();
    setMissing(view.missing);
    return view;
  }, []);

  /**
   * Reads files. With `stage`, their layers go into the list and wait for
   * Start, all chosen. Without it, the files are only read for the small
   * pictures of a job's layers, and a file that is gone is not an error.
   */
  const loadDocuments = useCallback(async (paths: string[], stage: boolean) => {
    setOpening(true);
    if (stage) setMessage(null);
    const opened: DocumentInfo[] = [];
    const errors: string[] = [];
    for (const path of paths) {
      try {
        const info = await openDocument(path);
        if (stage && !info.clipWidth) errors.push(`${baseName(path)} is wider or taller than 3:1; Wan 2.2 cannot make a clip of that shape.`);
        else opened.push(info);
      } catch (error) {
        errors.push(`${baseName(path)}: ${String(error)}`);
      }
    }
    setKnown((current) => {
      const next = new Map(current);
      for (const info of opened) next.set(fileKey(info.path), info);
      return next;
    });
    if (stage) {
      const keys = opened.flatMap((info) => info.layers.map((layer) => layerKey(info.path, layer.tile)));
      setStaged((current) => [...current, ...opened.map((info) => fileKey(info.path)).filter((file, index, all) => !current.includes(file) && all.indexOf(file) === index)]);
      setChosen((current) => new Set([...current, ...keys]));
      setRemoved((current) => new Set([...current].filter((key) => !keys.includes(key))));
      /* One photo has no prompt, so its text box opens at once. */
      const only = opened.length === 1 && opened[0].layers.length === 1 ? opened[0] : null;
      if (only && only.layers[0].tile === PHOTO_TILE) setEditing(`s:${layerKey(only.path, PHOTO_TILE)}`);
      if (errors.length) setMessage(errors.join(" "));
    }
    setOpening(false);
    return opened;
  }, []);

  /* Start: the version, an update check, the saved settings, and a job that was not finished when the app closed. */
  useEffect(() => {
    if (!isTauri()) return;
    void getAppVersion().then(setVersion);
    void checkForUpdate().then(setUpdateInfo).catch(() => { /* No network: the About dialog can check again. */ });
    void refreshSettings().then((view) => {
      setOutputDir(view.settings.outputDir);
      setClipsPerLayer(view.settings.clipsPerLayer || 2);
    });
    void (async () => {
      const saved = await queueGet();
      setSnapshot(saved);
      const requested = await takePendingOpenDocument();
      if (saved.job) await loadDocuments([...new Set(saved.job.layers.map((layer) => layer.document))], false);
      if (requested) await loadDocuments([requested], true);
    })();
  }, [loadDocuments, refreshSettings]);

  useEffect(() => {
    if (!isTauri()) return;
    const stops = [
      listen<Snapshot>("queue-changed", (event) => setSnapshot(event.payload)),
      listen<string>("clip-preview", (event) => setPreview(event.payload)),
      listen("close-requested", () => setCloseAsked(true)),
      listen<string>("open-document-requested", (event) => void loadDocuments([event.payload], true))
    ];
    return () => { for (const stop of stops) void stop.then((unlisten) => unlisten()); };
  }, [loadDocuments]);

  /* The picture of the clip in work goes away when that clip ends. */
  useEffect(() => {
    if (!snapshot.current || snapshot.current.stage === "preparing" || snapshot.current.stage === "loading") setPreview(null);
  }, [snapshot.current?.layer, snapshot.current?.clip, snapshot.current?.stage]);

  const chooseDocuments = async () => {
    const picked = await open({ multiple: true, title: "Open Image Sage documents and photos", filters: [
      { name: "Image Sage documents and photos", extensions: ["imagesage", "png", "jpg", "jpeg", "webp"] },
      { name: "Image Sage documents", extensions: ["imagesage"] },
      { name: "Photos", extensions: ["png", "jpg", "jpeg", "webp"] }
    ] });
    const paths = typeof picked === "string" ? [picked] : picked ?? [];
    if (paths.length) await loadDocuments(paths, true);
  };

  const chooseOutput = async () => {
    const picked = await open({ directory: true, defaultPath: outputDir || undefined, title: "Choose the folder for the videos" });
    if (typeof picked === "string") setOutputDir(picked);
  };

  /* Takes files out of the list of files that wait, with all that the window keeps for their layers. */
  const unstage = (files: Set<string>) => {
    const stays = (key: string) => !files.has(key.slice(0, key.lastIndexOf("|")));
    setStaged((current) => current.filter((file) => !files.has(file)));
    setChosen((current) => new Set([...current].filter(stays)));
    setRemoved((current) => new Set([...current].filter(stays)));
    setPrompts((current) => new Map([...current].filter(([key]) => stays(key))));
    setEditing(null);
  };

  /* Starts a job with the chosen layers, or adds them to the job that exists. Their files then leave the list of files that wait. */
  const start = async () => {
    const sent = stagedRows.filter((row) => chosen.has(row.key));
    if (!sent.length) return;
    setMessage(null);
    try {
      await queueStart({
        outputDir,
        clips: clipsPerLayer,
        layers: sent.map((row) => ({ document: row.path, tile: row.tile, prompt: prompts.has(row.key) ? row.prompt : null }))
      });
      unstage(new Set(sent.map((row) => fileKey(row.path))));
    } catch (error) {
      setMessage(String(error));
    }
  };

  /* Removes the job. Its files go back into the list with the job's prompts, so the user can change them and start again. */
  const newJob = async () => {
    const layers = job?.layers ?? [];
    try {
      await queueClear();
      setPreview(null);
      const opened = await loadDocuments([...new Set(layers.map((layer) => layer.document))], true);
      setPrompts((current) => {
        const next = new Map(current);
        for (const layer of layers) {
          const original = opened.find((info) => fileKey(info.path) === fileKey(layer.document))?.layers.find((known) => known.tile === layer.tile)?.prompt;
          if (original !== undefined && original !== layer.prompt) next.set(layerKey(layer.document, layer.tile), layer.prompt);
        }
        return next;
      });
    } catch (error) {
      setMessage(String(error));
    }
  };

  const clearDone = () => void queueClearDone().catch((error) => setMessage(String(error)));

  /* Closes the files that wait, and removes a job that does not run. The files and the finished videos do not change. */
  const clearDocuments = async () => {
    try {
      if (job && !running) await queueClear();
      unstage(new Set(staged));
      setPreview(null);
      setMessage(null);
    } catch (error) {
      setMessage(String(error));
    }
  };

  /**
   * Renames a file on the disk. The job gets the new path from the Rust
   * side. For a document or a photo, the window reads the file again and
   * moves all that it keeps for the file's layers to the new path.
   */
  const renameFile = async (path: string, name: string) => {
    try {
      const renamed = await fileRename(path, name);
      const [from, to] = [fileKey(path), fileKey(renamed)];
      if (renamed === path || !known.has(from)) return;
      const info = await openDocument(renamed);
      const moved = (key: string) => (key.startsWith(`${from}|`) ? `${to}|${key.slice(from.length + 1)}` : key);
      setKnown((current) => {
        const next = new Map(current);
        next.delete(from);
        next.set(to, info);
        return next;
      });
      setStaged((current) => current.map((file) => (file === from ? to : file)));
      setChosen((current) => new Set([...current].map(moved)));
      setRemoved((current) => new Set([...current].map(moved)));
      setPrompts((current) => new Map([...current].map(([key, text]) => [moved(key), text])));
      setEditing(null);
    } catch (error) {
      setMessage(String(error));
    }
  };

  /* Moves a file to the Recycle Bin. A document or a photo leaves the files that wait; the Rust side removes a video's layer from the job's list. */
  const deleteFile = async (path: string, video: boolean) => {
    setDeleting(null);
    try {
      await fileDelete(path);
      if (!video) unstage(new Set([fileKey(path)]));
    } catch (error) {
      setMessage(String(error));
    }
  };

  /* The buttons of one file: show it in its folder, rename it, delete it. */
  const fileActions = (path: string, video: boolean) => ({
    path,
    video,
    renaming: renaming === fileKey(path),
    onReveal: () => void revealPath(path).catch((error) => setMessage(String(error))),
    onRenameStart: () => setRenaming(fileKey(path)),
    onRename: (name: string) => void renameFile(path, name),
    onRenameEnd: () => setRenaming(null),
    onDelete: () => setDeleting({ path, video })
  });

  const layerInfos = useMemo(() => {
    const infos = new Map<string, DocumentInfo["layers"][number]>();
    for (const info of known.values()) for (const layer of info.layers) infos.set(layerKey(info.path, layer.tile), layer);
    return infos;
  }, [known]);

  const zoomLayer = (row: Row) => {
    const small = layerInfos.get(row.key)?.thumbnail;
    if (!small) return;
    setZoom({ id: row.id, name: row.name, src: small });
    layerPicture(row.path, row.tile)
      .then((src) => setZoom((current) => (current?.id === row.id ? { ...current, src } : current)))
      .catch((error) => setMessage(String(error)));
  };

  useEffect(() => {
    if (!zoom) return;
    const close = (event: KeyboardEvent) => { if (event.key === "Escape") setZoom(null); };
    window.addEventListener("keydown", close);
    return () => window.removeEventListener("keydown", close);
  }, [zoom !== null]);

  /* Keeps a changed prompt of a layer that waits. A prompt that is the same as the file's prompt is not a change. */
  const setPrompt = (key: string, text: string) => {
    const original = layerInfos.get(key)?.prompt ?? "";
    setPrompts((current) => {
      const next = new Map(current);
      if (text.trim() === original) next.delete(key);
      else next.set(key, text.trim());
      return next;
    });
  };

  /* For a layer that waits, the window keeps the prompt. For a job's layer, the job gets it at once. */
  const keepPrompt = (row: Row, text: string) => {
    if (row.jobLayer < 0) setPrompt(row.key, text);
    else if (text.trim() !== row.prompt) void queueSetPrompt(row.jobLayer, text).catch((error) => setMessage(String(error)));
  };

  /* Puts a row's prompt on the clipboard. This is the only way to get the prompt of a layer whose prompt cannot change. */
  const copyPrompt = (row: Row) => {
    navigator.clipboard.writeText(row.prompt)
      .then(() => {
        setCopied(row.id);
        window.setTimeout(() => setCopied((current) => (current === row.id ? null : current)), 1500);
      })
      .catch((error) => setMessage(`Could not copy the prompt: ${String(error)}`));
  };

  const toggle = (key: string) => setChosen((current) => {
    const next = new Set(current);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    return next;
  });

  const removeLayer = (key: string) => {
    setRemoved((current) => new Set(current).add(key));
    setChosen((current) => {
      const next = new Set(current);
      next.delete(key);
      return next;
    });
  };

  /* The layers that wait for Start: each file's layers, bottom layer first. */
  const stagedRows: Row[] = staged.flatMap((file) => {
    const info = known.get(file);
    return (info?.layers ?? []).map((layer, index) => {
      const key = layerKey(info!.path, layer.tile);
      return { id: `s:${key}`, key, path: info!.path, tile: layer.tile, jobLayer: -1, number: index + 1, name: layer.name, prompt: prompts.get(key) ?? layer.prompt, clips: null, video: null };
    });
  }).filter((row) => !removed.has(row.key));
  /* The job's layers that have a clip in the list. */
  const jobRows: Row[] = (job?.layers ?? []).map((layer, index) => ({
    id: `j:${index}`, key: layerKey(layer.document, layer.tile), path: layer.document, tile: layer.tile, jobLayer: index,
    number: layer.index + 1, name: layer.name, prompt: layer.prompt, clips: layer.clips, video: layer.video
  })).filter((row) => row.clips.some((clip) => !clip.cleared));
  /* The layers that wait are first, so the user sees the files that were opened last. */
  const shownRows = [...topLayerFirst(stagedRows), ...topLayerFirst(jobRows)];
  const chosenCount = stagedRows.filter((row) => chosen.has(row.key)).length;
  const hasRemoved = staged.length > 0 && removed.size > 0;

  /* The clips in the list. While the clips are joined, all of them are done and the list must not become empty. */
  const listedClips = jobRows.flatMap((row) => row.clips ?? []).filter((clip) => !clip.cleared);
  const doneClips = listedClips.filter((clip) => clip.status === "done").length;
  const canClearDone = doneClips > 0 && !(running && doneClips === listedClips.length);

  const stagedInfos = staged.map((file) => known.get(file)).filter((info): info is DocumentInfo => !!info);
  const jobFiles = new Set((job?.layers ?? []).map((layer) => fileKey(layer.document))).size;
  const overall = snapshot.clipsTotal
    ? (snapshot.clipsDone + snapshot.clipsFailed + (running && snapshot.current && snapshot.current.stage !== "joining" ? snapshot.current.progress : 0)) / snapshot.clipsTotal
    : 0;
  const canStart = chosenCount > 0 && missing.length === 0 && (!!job || outputDir.trim() !== "");
  const startHint = !stagedRows.length && !hasRemoved ? "Open Image Sage documents or photos first."
    : missing.length ? `Choose the Wan 2.2 ${missing.join(", ")} in Settings.`
    : !outputDir.trim() ? "Choose the folder for the videos."
    : chosenCount === 0 ? "Choose one layer or more."
    : `Make ${chosenCount} ${chosenCount === 1 ? "video" : "videos"} of ${clipsPerLayer * CLIP_SECONDS} seconds: ${clipsPerLayer} ${clipsPerLayer === 1 ? "clip" : "clips"} for each layer.`;

  const statusText = !job ? startHint
    : snapshot.state === "running" && snapshot.current
      ? snapshot.current.stage === "joining"
        ? "Joining the clips into one video…"
        : `${STAGE_TEXT[snapshot.current.stage]} — clip ${snapshot.clipsDone + snapshot.clipsFailed + 1} of ${snapshot.clipsTotal}`
    : snapshot.state === "paused"
      ? `Paused. ${snapshot.clipsDone} of ${snapshot.clipsTotal} clips are done. The model is not in the GPU memory.`
    : `Finished. ${snapshot.clipsDone} of ${snapshot.clipsTotal} clips are done${snapshot.clipsFailed ? `, ${snapshot.clipsFailed} failed` : ""}.`;

  /* The picture of the clip in work has the shape of that clip's layer. */
  const previewLayer = job ? job.layers[snapshot.current?.layer ?? 0] ?? job.layers[0] : null;

  return (
    <div className="app-shell">
      <TitleBar updateAvailable={!!updateInfo} onAbout={() => setAboutOpen(true)} onSettings={() => setSettingsOpen(true)} />

      <section className="setup">
        <div className="setup-row">
          <span className="setup-label">Files</span>
          <div className="path-box" title={stagedInfos.map((info) => info.path).join("\n")}>
            {stagedInfos.length === 1 ? baseName(stagedInfos[0].path)
              : stagedInfos.length > 1 ? `${stagedInfos.length} files: ${stagedInfos.map((info) => baseName(info.path)).join(", ")}`
              : job ? `${jobFiles} ${jobFiles === 1 ? "file is" : "files are"} in the job`
              : <em>No document or photo is open</em>}
            {stagedInfos.length === 1 && <small>{stagedInfos[0].width} × {stagedInfos[0].height} · clips {stagedInfos[0].clipWidth} × {stagedInfos[0].clipHeight}, 5 s</small>}
          </div>
          <div className="button-group">
            <button className="button secondary" onClick={() => void chooseDocuments()} disabled={opening} title={job ? "Open more Image Sage documents or photos, to add them to the job" : "Open Image Sage documents or photos. You can choose many files."}>
              {opening ? <CircleNotch className="spin" size={15} /> : <FolderOpen size={15} />} Open…
            </button>
            <button
              className="button secondary"
              onClick={() => void clearDocuments()}
              disabled={staged.length === 0 && (!job || running)}
              title={job && running ? "Close the files that are not in the job. The job goes on."
                : job ? "Close the files and remove the job from the list. The finished videos stay in their folder."
                : "Close the files. The files do not change."}
            >
              <X size={15} /> Clear
            </button>
          </div>
        </div>
        <div className="setup-row">
          <span className="setup-label">Save videos in</span>
          <div className="path-box" title={job?.outputDir ?? outputDir}>
            {job ? job.outputDir : outputDir || <em>No folder is chosen</em>}
          </div>
          {job ? (
            <button className="button secondary" onClick={() => void revealPath(job.outputDir).catch((error) => setMessage(String(error)))} title="Show the folder in File Explorer">
              <FolderOpen size={15} /> Show
            </button>
          ) : (
            <button className="button secondary" onClick={() => void chooseOutput()}><FolderOpen size={15} /> Choose…</button>
          )}
        </div>
        <div className="setup-row">
          <label className="setup-label" htmlFor="clips-per-layer">Clips per layer</label>
          <div className="setup-clips" title="Each clip is 5 seconds. The first clip starts from the layer's picture. Each clip after it starts from the last picture of the clip before. The clips are then joined into one video, and the clips are deleted. This number is for the layers that you start or add next.">
            <input
              id="clips-per-layer"
              type="number"
              min={1}
              max={MAX_CLIPS_PER_LAYER}
              value={clipsPerLayer}
              onChange={(event) => setClipsPerLayer(Math.min(MAX_CLIPS_PER_LAYER, Math.max(1, Math.round(Number(event.target.value)) || 1)))}
            />
            <span>= one video of {clipsPerLayer * CLIP_SECONDS} seconds for each layer{job ? " that you add" : ""}</span>
          </div>
        </div>
      </section>

      {missing.length > 0 && (!job || stagedRows.length > 0) && (
        <div className="banner">
          <WarningCircle size={16} weight="fill" />
          <span>Wan 2.2 is not set up: the {missing.join(", ")} {missing.length === 1 ? "is" : "are"} missing.</span>
          <button className="link-button" onClick={() => setSettingsOpen(true)}>Open Settings</button>
        </div>
      )}

      <main className="workspace">
        <div className="layers">
          {(stagedRows.length > 1 || hasRemoved || (!!job && stagedRows.length > 0)) && (
            <div className="layers-toolbar">
              <span>{chosenCount} of {stagedRows.length} layers{job ? " to add to the job" : ""}</span>
              {hasRemoved && <button className="link-button" onClick={() => setRemoved(new Set())} title="Show the layers that you removed from the list">Show removed ({removed.size})</button>}
              <button className="link-button" onClick={() => setChosen(new Set(stagedRows.map((row) => row.key)))}>All</button>
              <button className="link-button" onClick={() => setChosen(new Set())}>None</button>
            </div>
          )}
          <div className="layers-list">
            {shownRows.length === 0 && !hasRemoved && (
              <div className="empty">
                <FilmStrip size={44} weight="thin" />
                <p>Open Image Sage documents or photos.</p>
                <span>Each layer becomes one Wan 2.2 video, made of 5-second clips. The first clip starts from the layer's picture; each clip after it continues the clip before. All clips follow the layer's prompt. A photo (PNG, JPEG or WebP) is one layer; you write its prompt here. You can open more files while a job runs.</span>
              </div>
            )}
            {shownRows.map((row, place) => {
              const waits = row.jobLayer < 0;
              /* Each file's rows have the file's name above them. A file can be in the list two times: its layers that wait, and its layers in the job. */
              const before = shownRows[place - 1];
              const firstOfFile = !before || fileKey(before.path) !== fileKey(row.path) || (before.jobLayer < 0) !== waits;
              const info = layerInfos.get(row.key);
              /* A layer that waits can always get a new prompt. A job's layer can get one while it has a clip that did not start yet. */
              const canEdit = waits || (!row.video && !!row.clips?.some((clip) => clip.status === "pending" || clip.status === "failed"));
              return (
                <Fragment key={row.id}>
                {firstOfFile && <FileLine header {...fileActions(row.path, false)} />}
                <div className={`layer-row ${waits && !chosen.has(row.key) ? "off" : ""}`}>
                  {waits && <input type="checkbox" checked={chosen.has(row.key)} onChange={() => toggle(row.key)} onClick={(event) => event.stopPropagation()} aria-label={`Make clips of ${row.name}`} />}
                  {info?.thumbnail ? (
                    <button className="layer-thumb zoomable" title="Show this picture large" aria-label={`Show the picture of ${row.name} large`} onClick={(event) => { event.stopPropagation(); zoomLayer(row); }}>
                      <img src={info.thumbnail} alt="" />
                    </button>
                  ) : (
                    <div className="layer-thumb"><FilmStrip size={20} /></div>
                  )}
                  <div className="layer-text">
                    <div className="layer-name">
                      {row.tile !== PHOTO_TILE && <b>{row.number}</b>} {row.name}
                      {info?.hidden && <span className="layer-hidden" title="This layer is turned off in Image Sage"><EyeSlash size={13} /></span>}
                    </div>
                    {canEdit && editing === row.id ? (
                      <div className="layer-prompt-edit" onClick={(event) => event.stopPropagation()}>
                        <textarea
                          rows={3}
                          autoFocus
                          spellCheck={false}
                          defaultValue={row.prompt}
                          placeholder="No prompt: Wan 2.2 chooses the motion."
                          aria-label={`The prompt of ${row.name}`}
                          onFocus={(event) => { editCancelled.current = false; event.currentTarget.select(); }}
                          onBlur={(event) => {
                            if (!editCancelled.current) keepPrompt(row, event.currentTarget.value);
                            editCancelled.current = false;
                            setEditing(null);
                          }}
                          onKeyDown={(event) => {
                            if (event.key === "Escape") { editCancelled.current = true; event.currentTarget.blur(); }
                            else if (event.key === "Enter" && !event.shiftKey) { event.preventDefault(); event.currentTarget.blur(); }
                          }}
                        />
                        <small>
                          Enter keeps the prompt. Esc cancels. {waits ? "The file does not change." : "The clips of this layer that start after this use the new prompt."}
                          {waits && prompts.has(row.key) && (
                            <button
                              className="link-button"
                              onMouseDown={(event) => event.preventDefault()}
                              onClick={() => { editCancelled.current = true; setPrompt(row.key, info?.prompt ?? ""); setEditing(null); }}
                            >
                              Restore the file's prompt
                            </button>
                          )}
                        </small>
                      </div>
                    ) : (
                      <div className="layer-prompt" title={row.prompt}>
                        {row.prompt || "No prompt: Wan 2.2 chooses the motion."}
                        {waits && prompts.has(row.key) && <em> (changed)</em>}
                      </div>
                    )}
                    {row.video && <FileLine {...fileActions(row.video, true)} />}
                  </div>
                  {row.video && (
                    <div className="clips">
                      <button
                        className="clip done video"
                        title={`Play ${baseName(row.video)}`}
                        onClick={(event) => { event.stopPropagation(); void playVideo(row.video!).catch((error) => setMessage(String(error))); }}
                      >
                        <Play size={11} weight="fill" /> {(row.clips?.length ?? 0) * CLIP_SECONDS} s
                      </button>
                    </div>
                  )}
                  {row.clips && !row.video && (
                    <div className="clips">
                      {row.clips.map((clip, index) => !clip.cleared && (
                        <ClipChip
                          key={index}
                          number={index + 1}
                          clip={clip}
                          current={snapshot.current && snapshot.current.stage !== "joining" && snapshot.current.layer === row.jobLayer && snapshot.current.clip === index ? snapshot.current : null}
                          onError={setMessage}
                        />
                      ))}
                    </div>
                  )}
                  {row.prompt && (
                    <button
                      className="layer-remove"
                      title={copied === row.id ? "The prompt is on the clipboard" : "Copy this layer's prompt"}
                      aria-label={`Copy the prompt of ${row.name}`}
                      onClick={(event) => { event.stopPropagation(); copyPrompt(row); }}
                    >
                      {copied === row.id ? <CheckCircle size={15} weight="fill" /> : <Copy size={15} />}
                    </button>
                  )}
                  {canEdit && (
                    <button
                      className="layer-remove"
                      title={waits ? "Change this layer's prompt for the video. The file does not change." : "Change this layer's prompt. The clips of this layer that start after this use the new prompt."}
                      aria-label={`Change the prompt of ${row.name}`}
                      onClick={(event) => { event.stopPropagation(); setEditing(row.id); }}
                    >
                      <PencilSimple size={15} />
                    </button>
                  )}
                  {waits && (
                    <button
                      className="layer-remove"
                      title="Remove this layer from the list. The file does not change."
                      aria-label={`Remove ${row.name} from the list`}
                      onClick={(event) => { event.stopPropagation(); removeLayer(row.key); }}
                    >
                      <X size={15} />
                    </button>
                  )}
                </div>
                </Fragment>
              );
            })}
          </div>
        </div>

        {job && previewLayer && (
          <aside className="preview">
            <div className="preview-frame" style={{ aspectRatio: `${previewLayer.clipWidth || 16} / ${previewLayer.clipHeight || 9}` }}>
              {preview ? <img src={preview} alt="The clip in work" /> : <FilmStrip size={36} weight="thin" />}
            </div>
            <div className="preview-caption">
              {running && snapshot.current
                ? snapshot.current.stage === "joining" ? "Joining the clips" : `${job.layers[snapshot.current.layer]?.name ?? ""} — clip ${snapshot.current.clip + 1}`
                : snapshot.state === "paused" ? "Paused" : "Finished"}
            </div>
          </aside>
        )}
      </main>

      {(message || snapshot.error) && (
        <div className="banner error">
          <WarningCircle size={16} weight="fill" />
          <span>{message ?? snapshot.error}</span>
          {message && <button className="link-button" onClick={() => setMessage(null)}>Dismiss</button>}
        </div>
      )}

      <footer className="statusbar">
        <div className="status">
          <div className="status-text">
            {running && <CircleNotch className="spin" size={14} />}
            {snapshot.state === "finished" && <CheckCircle size={15} weight="fill" />}
            <span>{statusText}</span>
          </div>
          {job && <div className="progress-track"><i style={{ width: `${Math.round(overall * 1000) / 10}%` }} /></div>}
        </div>
        <div className="status-actions">
          {!job && (
            <button className="button primary large" onClick={() => void start()} disabled={!canStart}>
              <Play size={16} weight="fill" /> Start
            </button>
          )}
          {job && stagedRows.length > 0 && (
            <button
              className="button primary large"
              onClick={() => void start()}
              disabled={!canStart}
              title={chosenCount === 0 ? "Choose one layer or more."
                : `Add ${chosenCount} ${chosenCount === 1 ? "layer" : "layers"} to the job, with ${clipsPerLayer} ${clipsPerLayer === 1 ? "clip" : "clips"} for each layer. ${snapshot.state === "paused" ? "The job stays paused." : "The job makes them after its other layers."}`}
            >
              <Plus size={16} weight="bold" /> Add to Job
            </button>
          )}
          {job && running && (
            <button className={`button large ${stagedRows.length > 0 ? "secondary" : "primary"}`} onClick={() => void queuePause()} title="Stop the clip in work and remove the model from the GPU memory. Resume makes that clip again from its start.">
              <Pause size={16} weight="fill" /> Pause
            </button>
          )}
          {job && running && (
            <button
              className="button secondary large"
              onClick={() => void queueCancelClip().catch((error) => setMessage(String(error)))}
              disabled={!snapshot.current || snapshot.current.stage === "joining"}
              title="Stop the clip in work and do not make it. The layer's video gets one clip less, and the job goes on with the next clip. The model stays in the GPU memory."
            >
              <X size={16} /> Cancel Clip
            </button>
          )}
          {job && snapshot.state === "paused" && (
            <button className={`button large ${stagedRows.length > 0 ? "secondary" : "primary"}`} onClick={() => void queueResume()} title="Load the model and go on with the job">
              <Play size={16} weight="fill" /> Resume
            </button>
          )}
          {job && snapshot.state === "finished" && snapshot.clipsFailed > 0 && (
            <button className="button secondary large" onClick={() => void queueResume()} title="Make the failed clips again">
              <Play size={16} weight="fill" /> Retry Failed Clips
            </button>
          )}
          {job && (
            <button
              className="button secondary large"
              onClick={clearDone}
              disabled={!canClearDone}
              title={canClearDone
                ? "Remove the finished clips and videos from the list. The videos stay in the folder."
                : doneClips > 0 ? "Wait until the clips are joined." : "No clip is finished yet."}
            >
              <Trash size={16} /> Clear Finished
            </button>
          )}
          {job && !running && (
            <button className={`button large ${snapshot.state === "finished" && stagedRows.length === 0 ? "primary" : "secondary"}`} onClick={() => void newJob()} title="Remove this job from the list and put its files back into the list. The finished videos stay in their folder.">
              <Plus size={16} weight="bold" /> New Job
            </button>
          )}
        </div>
      </footer>

      {settingsOpen && <SettingsDialog onClose={() => { setSettingsOpen(false); void refreshSettings(); }} />}
      {aboutOpen && <AboutDialog version={version} initialUpdateInfo={updateInfo} onClose={() => setAboutOpen(false)} />}
      {zoom && (
        <div className="zoom-overlay" role="dialog" aria-modal="true" aria-label={zoom.name} title="Click to close" onClick={() => setZoom(null)}>
          <img src={zoom.src} alt={zoom.name} />
        </div>
      )}
      {deleting && (
        <div className="dialog-overlay" role="presentation" onPointerDown={() => setDeleting(null)}>
          <div className="dialog confirm-dialog" role="dialog" aria-modal="true" aria-labelledby="delete-title" onPointerDown={(event) => event.stopPropagation()}>
            <div className="dialog-body">
              <h2 id="delete-title">Delete {baseName(deleting.path)}?</h2>
              <p>
                The file goes to the Recycle Bin. {deleting.video
                  ? "The video leaves the list. Its document or photo does not change."
                  : "Its layers that wait for Start leave the list. The finished videos do not change."}
              </p>
              <p className="dialog-path">{deleting.path}</p>
            </div>
            <footer className="dialog-footer">
              <button className="button secondary" onClick={() => setDeleting(null)}>Cancel</button>
              <button className="button primary" autoFocus onClick={() => void deleteFile(deleting.path, deleting.video)}>Delete</button>
            </footer>
          </div>
        </div>
      )}
      {closeAsked && (
        <div className="dialog-overlay" role="presentation" onPointerDown={() => setCloseAsked(false)}>
          <div className="dialog confirm-dialog" role="dialog" aria-modal="true" aria-labelledby="close-title" onPointerDown={(event) => event.stopPropagation()}>
            <div className="dialog-body">
              <h2 id="close-title">A job is running</h2>
              <p>Hide the window to the tray and the job goes on. If you quit, the clip in work is lost; the rest of the job can go on the next time you start Image Sage Video.</p>
            </div>
            <footer className="dialog-footer">
              <button className="button secondary" onClick={() => setCloseAsked(false)}>Cancel</button>
              <button className="button secondary" onClick={() => void quitApp()}>Quit</button>
              <button className="button primary" onClick={() => { setCloseAsked(false); void hideToTray(); }}>Hide to Tray</button>
            </footer>
          </div>
        </div>
      )}
    </div>
  );
}

interface FileLineProps {
  path: string;
  /** The file is a finished video; if not, it is a document or a photo. */
  video: boolean;
  /** The line is above a file's rows; if not, it is in a row. */
  header?: boolean;
  /** The file's name is in the text box now. */
  renaming: boolean;
  onReveal: () => void;
  onRenameStart: () => void;
  onRename: (name: string) => void;
  onRenameEnd: () => void;
  onDelete: () => void;
}

/**
 * A file's name with its buttons: show the file in its folder, rename it,
 * delete it. For a new name, the user writes the name without the
 * extension. Enter or a click out of the box keeps the name; Esc cancels.
 */
function FileLine({ path, video, header, renaming, onReveal, onRenameStart, onRename, onRenameEnd, onDelete }: FileLineProps) {
  const cancelled = useRef(false);
  const name = baseName(path);
  const dot = name.lastIndexOf(".");
  const stem = dot > 0 ? name.slice(0, dot) : name;
  const extension = dot > 0 ? name.slice(dot) : "";
  const kind = video ? "video" : "file";
  return (
    <div className={`file-line ${header ? "header" : ""} ${video ? "video" : ""}`} onClick={(event) => event.stopPropagation()}>
      {video ? <FilmStrip size={13} /> : <FileIcon size={13} />}
      {renaming ? (
        <>
          <input
            type="text"
            autoFocus
            spellCheck={false}
            defaultValue={stem}
            aria-label={`The new name of ${name}`}
            onFocus={(event) => { cancelled.current = false; event.currentTarget.select(); }}
            onBlur={(event) => {
              const next = event.currentTarget.value.trim();
              if (!cancelled.current && next && next !== stem) onRename(next);
              cancelled.current = false;
              onRenameEnd();
            }}
            onKeyDown={(event) => {
              if (event.key === "Escape") { cancelled.current = true; event.currentTarget.blur(); }
              else if (event.key === "Enter") { event.preventDefault(); event.currentTarget.blur(); }
            }}
          />
          <span className="file-extension">{extension}</span>
        </>
      ) : (
        <span className="file-name" title={path}>{name}</span>
      )}
      <button className="file-action" title={`Show this ${kind} in its folder`} aria-label={`Show ${name} in its folder`} onClick={onReveal}>
        <FolderOpen size={14} />
      </button>
      <button className="file-action" title={`Rename this ${kind} on the disk`} aria-label={`Rename ${name}`} onClick={onRenameStart}>
        <PencilSimple size={14} />
      </button>
      <button className="file-action" title={`Delete this ${kind}: move it to the Recycle Bin`} aria-label={`Delete ${name}`} onClick={onDelete}>
        <Trash size={14} />
      </button>
    </div>
  );
}

/** One clip of a layer: waiting, in work with its progress, done (a click plays it, while it has a file), or failed (the tooltip gives the reason). */
function ClipChip({ number, clip, current, onError }: { number: number; clip: Clip; current: Current | null; onError: (message: string) => void }) {
  if (clip.status === "done" && clip.file) {
    const file = clip.file;
    return (
      <button className="clip done" title={`Play ${baseName(file)}`} onClick={(event) => { event.stopPropagation(); void playVideo(file).catch((error) => onError(String(error))); }}>
        <Play size={11} weight="fill" /> {number}
      </button>
    );
  }
  if (clip.status === "done") {
    return <span className="clip done joined" title="Done"><CheckCircle size={12} weight="fill" /> {number}</span>;
  }
  if (clip.status === "failed") {
    return <span className="clip failed" title={clip.error ?? "The clip failed"}><WarningCircle size={12} weight="fill" /> {number}</span>;
  }
  if (clip.status === "running") {
    const percent = Math.round((current?.progress ?? 0) * 100);
    return (
      <span className="clip running" title={current ? STAGE_TEXT[current.stage] : "In work"}>
        <i style={{ width: `${percent}%` }} />
        <em>{current?.stage === "generating" ? `${percent}%` : <CircleNotch className="spin" size={11} />}</em>
      </span>
    );
  }
  return <span className="clip pending" title="Waiting">{number}</span>;
}
