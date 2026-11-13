import { isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import {
  CheckCircle,
  CircleNotch,
  EyeSlash,
  FilmStrip,
  FolderOpen,
  Pause,
  Play,
  Plus,
  Trash,
  WarningCircle,
  X
} from "@phosphor-icons/react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { AboutDialog } from "./components/AboutDialog";
import { SettingsDialog } from "./components/SettingsDialog";
import { TitleBar } from "./components/TitleBar";
import {
  baseName,
  hideToTray,
  openDocument,
  playVideo,
  queueClear,
  queueClearDone,
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
const EMPTY: Snapshot = { job: null, state: "idle", current: null, error: null, clipsDone: 0, clipsFailed: 0, clipsTotal: 0 };

const STAGE_TEXT: Record<Current["stage"], string> = {
  preparing: "Preparing the layer's picture",
  loading: "Loading the model",
  generating: "Making the clip",
  finishing: "Finishing the clip",
  joining: "Joining the clips"
};

/**
 * The one window of Image Sage Video. Before a job, it shows the opened
 * document's layers with a checkbox for each. During a job, it shows the
 * clips of each layer and their progress, then each layer's video. The job itself runs in the
 * Rust side, so it goes on while this window is hidden in the tray.
 */
export default function App() {
  const [version, setVersion] = useState("");
  const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null);
  const [aboutOpen, setAboutOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [closeAsked, setCloseAsked] = useState(false);

  const [snapshot, setSnapshot] = useState<Snapshot>(EMPTY);
  const [info, setInfo] = useState<DocumentInfo | null>(null);
  const [opening, setOpening] = useState(false);
  const [chosen, setChosen] = useState<Set<string>>(new Set());
  /* The layers the user removed from the list. The document does not change, and they come back when it is opened again. */
  const [removed, setRemoved] = useState<Set<string>>(new Set());
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

  const loadDocument = useCallback(async (path: string, selectAll: boolean) => {
    setOpening(true);
    setMessage(null);
    try {
      const opened = await openDocument(path);
      setInfo(opened);
      setRemoved(new Set());
      if (selectAll) setChosen(new Set(opened.layers.map((layer) => layer.tile)));
    } catch (error) {
      setMessage(String(error));
    } finally {
      setOpening(false);
    }
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
      if (saved.job) await loadDocument(saved.job.document, false);
      else if (requested) await loadDocument(requested, true);
    })();
  }, [loadDocument, refreshSettings]);

  useEffect(() => {
    if (!isTauri()) return;
    const stops = [
      listen<Snapshot>("queue-changed", (event) => setSnapshot(event.payload)),
      listen<string>("clip-preview", (event) => setPreview(event.payload)),
      listen("close-requested", () => setCloseAsked(true)),
      listen<string>("open-document-requested", (event) => {
        void queueGet().then((now) => {
          if (now.job) setMessage("Finish the job, or choose New Job, before you open another document.");
          else void loadDocument(event.payload, true);
        });
      })
    ];
    return () => { for (const stop of stops) void stop.then((unlisten) => unlisten()); };
  }, [loadDocument]);

  /* The picture of the clip in work goes away when that clip ends. */
  useEffect(() => {
    if (!snapshot.current || snapshot.current.stage === "preparing" || snapshot.current.stage === "loading") setPreview(null);
  }, [snapshot.current?.layer, snapshot.current?.clip, snapshot.current?.stage]);

  const chooseDocument = async () => {
    const picked = await open({ multiple: false, filters: [{ name: "Image Sage documents", extensions: ["imagesage"] }] });
    if (typeof picked === "string") await loadDocument(picked, true);
  };

  const chooseOutput = async () => {
    const picked = await open({ directory: true, defaultPath: outputDir || undefined, title: "Choose the folder for the videos" });
    if (typeof picked === "string") setOutputDir(picked);
  };

  const start = async () => {
    if (!info) return;
    setMessage(null);
    try {
      await queueStart({ document: info.path, outputDir, clips: clipsPerLayer, tiles: [...chosen] });
    } catch (error) {
      setMessage(String(error));
    }
  };

  const newJob = async () => {
    try {
      await queueClear();
      setPreview(null);
      setMessage(null);
      if (info) setChosen(new Set(info.layers.filter((layer) => !removed.has(layer.tile)).map((layer) => layer.tile)));
    } catch (error) {
      setMessage(String(error));
    }
  };

  /* When no clip is left, the job is gone. The document then closes too, so the list is empty and does not show the layers again. */
  const clearDone = async () => {
    try {
      await queueClearDone();
      if ((await queueGet()).job) return;
      setInfo(null);
      setChosen(new Set());
      setPreview(null);
      setMessage(null);
    } catch (error) {
      setMessage(String(error));
    }
  };

  const toggle = (tile: string) => setChosen((current) => {
    const next = new Set(current);
    if (next.has(tile)) next.delete(tile);
    else next.add(tile);
    return next;
  });

  const removeLayer = (tile: string) => {
    setRemoved((current) => new Set(current).add(tile));
    setChosen((current) => {
      const next = new Set(current);
      next.delete(tile);
      return next;
    });
  };

  const listedLayers = (info?.layers ?? []).filter((layer) => !removed.has(layer.tile));

  const thumbnails = useMemo(() => new Map(info?.layers.map((layer) => [layer.tile, layer.thumbnail]) ?? []), [info]);
  const hiddenTiles = useMemo(() => new Set(info?.layers.filter((layer) => layer.hidden).map((layer) => layer.tile) ?? []), [info]);

  /* The rows: during a job, the job's layers; before it, the document's layers. Both lists have the bottom layer first; the list shows the top layer first, as Image Sage does. */
  const rows = job
    ? job.layers.filter((layer) => layer.clips.some((clip) => !clip.cleared)).map((layer) => ({ tile: layer.tile, number: layer.index + 1, name: layer.name, prompt: layer.prompt, clips: layer.clips as Clip[] | null, video: layer.video }))
    : (info?.layers ?? []).map((layer, index) => ({ tile: layer.tile, number: index + 1, name: layer.name, prompt: layer.prompt, clips: null as Clip[] | null, video: null as string | null })).filter((row) => !removed.has(row.tile));
  const shownRows = [...rows].reverse();
  /* The clips in the list. While the clips are joined, all of them are done and the list must not become empty. */
  const listedClips = rows.flatMap((row) => row.clips ?? []).filter((clip) => !clip.cleared);
  const doneClips = listedClips.filter((clip) => clip.status === "done").length;
  const canClearDone = doneClips > 0 && !(running && doneClips === listedClips.length);

  const clipSize = job ? `${job.clipWidth} × ${job.clipHeight}` : info?.clipWidth ? `${info.clipWidth} × ${info.clipHeight}` : null;
  const overall = snapshot.clipsTotal
    ? (snapshot.clipsDone + snapshot.clipsFailed + (running && snapshot.current && snapshot.current.stage !== "joining" ? snapshot.current.progress : 0)) / snapshot.clipsTotal
    : 0;
  const canStart = !!info && !!info.clipWidth && chosen.size > 0 && outputDir.trim() !== "" && missing.length === 0;
  const startHint = !info ? "Open an Image Sage document first."
    : !info.clipWidth ? "The document is wider or taller than 3:1; Wan 2.2 cannot make a clip of that shape."
    : missing.length ? `Choose the Wan 2.2 ${missing.join(", ")} in Settings.`
    : !outputDir.trim() ? "Choose the folder for the videos."
    : chosen.size === 0 ? "Choose one layer or more."
    : `Make ${chosen.size} ${chosen.size === 1 ? "video" : "videos"} of ${clipsPerLayer * CLIP_SECONDS} seconds: ${clipsPerLayer} ${clipsPerLayer === 1 ? "clip" : "clips"} for each layer.`;

  const statusText = !job ? startHint
    : snapshot.state === "running" && snapshot.current
      ? snapshot.current.stage === "joining"
        ? "Joining the clips into one video…"
        : `${STAGE_TEXT[snapshot.current.stage]} — clip ${snapshot.clipsDone + snapshot.clipsFailed + 1} of ${snapshot.clipsTotal}`
    : snapshot.state === "paused"
      ? `Paused. ${snapshot.clipsDone} of ${snapshot.clipsTotal} clips are done. The model is not in the GPU memory.`
    : `Finished. ${snapshot.clipsDone} of ${snapshot.clipsTotal} clips are done${snapshot.clipsFailed ? `, ${snapshot.clipsFailed} failed` : ""}.`;

  return (
    <div className="app-shell">
      <TitleBar updateAvailable={!!updateInfo} onAbout={() => setAboutOpen(true)} onSettings={() => setSettingsOpen(true)} />

      <section className="setup">
        <div className="setup-row">
          <span className="setup-label">Document</span>
          <div className="path-box" title={job?.document ?? info?.path ?? ""}>
            {job ? baseName(job.document) : info ? baseName(info.path) : <em>No document is open</em>}
            {info && <small>{info.width} × {info.height}{clipSize ? ` · clips ${clipSize}, 5 s` : ""}</small>}
          </div>
          <button className="button secondary" onClick={() => void chooseDocument()} disabled={!!job || opening} title={job ? "Choose New Job first" : "Open an Image Sage document"}>
            {opening ? <CircleNotch className="spin" size={15} /> : <FolderOpen size={15} />} Open…
          </button>
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
          <div className="setup-clips" title="Each clip is 5 seconds. The first clip starts from the layer's picture. Each clip after it starts from the last picture of the clip before. The clips are then joined into one video, and the clips are deleted.">
            <input
              id="clips-per-layer"
              type="number"
              min={1}
              max={MAX_CLIPS_PER_LAYER}
              value={job ? (job.layers[0]?.clips.length ?? clipsPerLayer) : clipsPerLayer}
              disabled={!!job}
              onChange={(event) => setClipsPerLayer(Math.min(MAX_CLIPS_PER_LAYER, Math.max(1, Math.round(Number(event.target.value)) || 1)))}
            />
            <span>= one video of {(job ? (job.layers[0]?.clips.length ?? clipsPerLayer) : clipsPerLayer) * CLIP_SECONDS} seconds for each layer</span>
          </div>
        </div>
      </section>

      {!job && missing.length > 0 && (
        <div className="banner">
          <WarningCircle size={16} weight="fill" />
          <span>Wan 2.2 is not set up: the {missing.join(", ")} {missing.length === 1 ? "is" : "are"} missing.</span>
          <button className="link-button" onClick={() => setSettingsOpen(true)}>Open Settings</button>
        </div>
      )}

      <main className="workspace">
        <div className="layers">
          {!job && info && (info.layers.length > 1 || removed.size > 0) && (
            <div className="layers-toolbar">
              <span>{chosen.size} of {listedLayers.length} layers</span>
              {removed.size > 0 && <button className="link-button" onClick={() => setRemoved(new Set())} title="Show the layers that you removed from the list">Show removed ({removed.size})</button>}
              <button className="link-button" onClick={() => setChosen(new Set(listedLayers.map((layer) => layer.tile)))}>All</button>
              <button className="link-button" onClick={() => setChosen(new Set())}>None</button>
            </div>
          )}
          <div className="layers-list">
            {shownRows.length === 0 && !(info && removed.size > 0) && (
              <div className="empty">
                <FilmStrip size={44} weight="thin" />
                <p>Open an Image Sage document.</p>
                <span>Each layer becomes one Wan 2.2 video, made of 5-second clips. The first clip starts from the layer's picture; each clip after it continues the clip before. All clips follow the layer's prompt.</span>
              </div>
            )}
            {shownRows.map((row) => {
              const jobLayer = job ? job.layers.findIndex((layer) => layer.tile === row.tile) : -1;
              return (
                <div className={`layer-row ${!job && !chosen.has(row.tile) ? "off" : ""}`} key={row.tile} onClick={() => !job && toggle(row.tile)}>
                  {!job && <input type="checkbox" checked={chosen.has(row.tile)} onChange={() => toggle(row.tile)} onClick={(event) => event.stopPropagation()} aria-label={`Make clips of ${row.name}`} />}
                  <div className="layer-thumb">{thumbnails.get(row.tile) ? <img src={thumbnails.get(row.tile)} alt="" /> : <FilmStrip size={20} />}</div>
                  <div className="layer-text">
                    <div className="layer-name">
                      <b>{row.number}</b> {row.name}
                      {hiddenTiles.has(row.tile) && <span className="layer-hidden" title="This layer is turned off in Image Sage"><EyeSlash size={13} /></span>}
                    </div>
                    <div className="layer-prompt" title={row.prompt}>{row.prompt || "No prompt: Wan 2.2 chooses the motion."}</div>
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
                          current={snapshot.current && snapshot.current.stage !== "joining" && snapshot.current.layer === jobLayer && snapshot.current.clip === index ? snapshot.current : null}
                          onError={setMessage}
                        />
                      ))}
                    </div>
                  )}
                  {!job && (
                    <button
                      className="layer-remove"
                      title="Remove this layer from the list. The document does not change."
                      aria-label={`Remove ${row.name} from the list`}
                      onClick={(event) => { event.stopPropagation(); removeLayer(row.tile); }}
                    >
                      <X size={15} />
                    </button>
                  )}
                </div>
              );
            })}
          </div>
        </div>

        {job && (
          <aside className="preview">
            <div className="preview-frame" style={{ aspectRatio: `${job.clipWidth} / ${job.clipHeight}` }}>
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
          {job && running && (
            <button className="button primary large" onClick={() => void queuePause()} title="Stop the clip in work and remove the model from the GPU memory. Resume makes that clip again from its start.">
              <Pause size={16} weight="fill" /> Pause
            </button>
          )}
          {job && snapshot.state === "paused" && (
            <button className="button primary large" onClick={() => void queueResume()} title="Load the model and go on with the job">
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
              onClick={() => void clearDone()}
              disabled={!canClearDone}
              title={canClearDone
                ? "Remove the finished clips and videos from the list. The videos stay in the folder."
                : doneClips > 0 ? "Wait until the clips are joined." : "No clip is finished yet."}
            >
              <Trash size={16} /> Clear Finished
            </button>
          )}
          {job && !running && (
            <button className={`button large ${snapshot.state === "finished" ? "primary" : "secondary"}`} onClick={() => void newJob()} title="Remove this job from the list. The finished videos stay in their folder.">
              <Plus size={16} weight="bold" /> New Job
            </button>
          )}
        </div>
      </footer>

      {settingsOpen && <SettingsDialog onClose={() => { setSettingsOpen(false); void refreshSettings(); }} />}
      {aboutOpen && <AboutDialog version={version} initialUpdateInfo={updateInfo} onClose={() => setAboutOpen(false)} />}
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
