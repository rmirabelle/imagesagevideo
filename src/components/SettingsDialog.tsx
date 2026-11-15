import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { CheckCircle, CircleNotch, DotsSixVertical, DownloadSimple, FolderOpen, MagnifyingGlass, Plus, Trash, WarningCircle, X } from "@phosphor-icons/react";
import { useEffect, useRef, useState, type PointerEvent } from "react";
import { LoraDialog } from "./LoraDialog";
import { StrengthInput } from "./StrengthInput";
import {
  baseName,
  engineDownload,
  engineStatus,
  findModels,
  folderOf,
  settingsFromImageSage,
  settingsGet,
  settingsSet,
  type EngineStatus,
  type LoraPair,
  type ModelFiles,
  type SettingsView
} from "../lib/api";

interface Props {
  onClose: () => void;
}

type FileKey = "highNoiseModel" | "diffusionModel" | "textEncoder" | "vae";

const MODEL_ROWS: { key: FileKey; label: string; help: string }[] = [
  { key: "highNoiseModel", label: "High-noise model", help: "Wan 2.2 image-to-video, high noise. It starts each clip." },
  { key: "diffusionModel", label: "Low-noise model", help: "Wan 2.2 image-to-video, low noise. It finishes each clip." },
  { key: "textEncoder", label: "Text encoder", help: "The UMT5-XXL text encoder." },
  { key: "vae", label: "VAE", help: "The Wan 2.1 VAE." }
];

const LORA_SIDES = [
  { file: "highNoise", strength: "highNoiseStrength", off: "highNoiseOff", label: "High", pass: "high-noise" },
  { file: "lowNoise", strength: "lowNoiseStrength", off: "lowNoiseOff", label: "Low", pass: "low-noise" }
] as const;

const MODEL_FILTER = [{ name: "Model files", extensions: ["safetensors", "gguf", "sft"] }];

/**
 * Settings: the local engine, and Wan 2.2 image-to-video's model files.
 * The model files are your own (for example from ComfyUI); the app reads
 * them in place. The settings are saved in an encrypted file.
 */
export function SettingsDialog({ onClose }: Props) {
  const [view, setView] = useState<SettingsView | null>(null);
  const [files, setFiles] = useState<ModelFiles | null>(null);
  const [engine, setEngine] = useState<EngineStatus | null>(null);
  const [download, setDownload] = useState<{ stage: string; progress: number } | null>(null);
  const [notice, setNotice] = useState<{ error: boolean; text: string } | null>(null);
  const [saving, setSaving] = useState(false);
  const [adding, setAdding] = useState(false);
  const [loraFolder, setLoraFolder] = useState("");
  const [dragging, setDragging] = useState<number | null>(null);
  const dragFrom = useRef<number | null>(null);
  const loraList = useRef<HTMLDivElement>(null);

  useEffect(() => {
    void settingsGet().then((loaded) => {
      setView(loaded);
      setFiles(loaded.settings.files);
      const last = loaded.settings.files.loras.at(-1);
      setLoraFolder(loaded.settings.loraFolder || folderOf(last?.highNoise || last?.lowNoise || ""));
    });
    void engineStatus().then(setEngine);
  }, []);

  useEffect(() => {
    const unlisten = listen<{ stage: string; progress: number }>("engine-download", (event) => setDownload(event.payload));
    return () => { void unlisten.then((stop) => stop()); };
  }, []);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || download) return;
      if (adding) setAdding(false);
      else onClose();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onClose, download, adding]);

  const change = (patch: Partial<ModelFiles>) => setFiles((current) => (current ? { ...current, ...patch } : current));

  const browse = async (key: FileKey) => {
    const picked = await open({ multiple: false, filters: MODEL_FILTER, defaultPath: files?.[key] || undefined });
    if (typeof picked === "string") change({ [key]: picked });
  };

  const changeLora = (index: number, patch: Partial<LoraPair>) =>
    setFiles((current) => (current ? { ...current, loras: current.loras.map((pair, at) => (at === index ? { ...pair, ...patch } : pair)) } : current));

  const removeLora = (index: number) =>
    setFiles((current) => (current ? { ...current, loras: current.loras.filter((_, at) => at !== index) } : current));

  const addLora = (pair: LoraPair) => {
    setFiles((current) => (current ? { ...current, loras: [...current.loras, pair] } : current));
    setAdding(false);
  };

  /** Keeps the folder of the last chosen LoRA file at once, so the file browser opens there also after Cancel. */
  const rememberLoraFolder = async (folder: string) => {
    setLoraFolder(folder);
    try {
      const latest = await settingsGet();
      await settingsSet({ ...latest.settings, loraFolder: folder });
    } catch {
      /* The folder only makes the next choice quicker. */
    }
  };

  /**
   * A LoRA moves while its handle is held: when the pointer comes onto
   * another row, the LoRA takes that row's position. Pointer events do this,
   * because the window takes the browser's own drag and drop for files.
   */
  const startDrag = (event: PointerEvent<HTMLButtonElement>, index: number) => {
    event.currentTarget.setPointerCapture(event.pointerId);
    dragFrom.current = index;
    setDragging(index);
  };

  const moveDrag = (event: PointerEvent<HTMLButtonElement>) => {
    const from = dragFrom.current;
    if (from === null || !loraList.current) return;
    const to = [...loraList.current.children].findIndex((row) => {
      const box = row.getBoundingClientRect();
      return event.clientY >= box.top && event.clientY < box.bottom;
    });
    if (to < 0 || to === from) return;
    dragFrom.current = to;
    setDragging(to);
    setFiles((current) => {
      if (!current) return current;
      const loras = [...current.loras];
      const [moved] = loras.splice(from, 1);
      loras.splice(to, 0, moved);
      return { ...current, loras };
    });
  };

  const endDrag = () => {
    dragFrom.current = null;
    setDragging(null);
  };

  const copyFromImageSage = async () => {
    try {
      setFiles(await settingsFromImageSage());
      setNotice({ error: false, text: "The model files are copied from Image Sage. Save to keep them." });
    } catch (error) {
      setNotice({ error: true, text: String(error) });
    }
  };

  const findInFolder = async () => {
    const folder = await open({ directory: true, title: "Choose your ComfyUI folder, or its models folder" });
    if (typeof folder !== "string") return;
    const found = await findModels(folder);
    const patch: Partial<ModelFiles> = {};
    for (const key of ["diffusionModel", "highNoiseModel", "textEncoder", "vae"] as const) {
      const path = found[key];
      if (path) patch[key] = path;
    }
    const count = Object.keys(patch).length;
    change(patch);
    setNotice(count
      ? { error: false, text: `${count} of 4 model files were found. Save to keep them.` }
      : { error: true, text: "No Wan 2.2 image-to-video files were found in that folder." });
  };

  const startDownload = async () => {
    setDownload({ stage: "downloading", progress: 0 });
    setNotice(null);
    try {
      await engineDownload();
    } catch (error) {
      setNotice({ error: true, text: String(error) });
    }
    setDownload(null);
    setEngine(await engineStatus());
  };

  const save = async () => {
    if (!view || !files) return;
    setSaving(true);
    try {
      /* The newest saved settings keep the output folder that a job may have set while this dialog was open. */
      const latest = await settingsGet();
      await settingsSet({ ...latest.settings, files });
      onClose();
    } catch (error) {
      setNotice({ error: true, text: String(error) });
      setSaving(false);
    }
  };

  return (
    <>
    <div className="dialog-overlay" role="presentation" onPointerDown={() => !download && onClose()}>
      <div className="dialog settings-dialog" role="dialog" aria-modal="true" aria-labelledby="settings-title" onPointerDown={(event) => event.stopPropagation()}>
        <header className="dialog-header">
          <h2 id="settings-title">Settings</h2>
          {!download && <button className="dialog-close" onClick={onClose} aria-label="Close Settings"><X size={18} /></button>}
        </header>
        <div className="dialog-body">
          <section className="settings-section">
            <h3>Local engine</h3>
            {!engine ? (
              <div className="settings-line"><CircleNotch className="spin" size={15} /> Checking…</div>
            ) : engine.installed ? (
              <div className="settings-line ok">
                <CheckCircle size={16} weight="fill" />
                {engine.sharedWithImageSage ? "Installed. Image Sage Video uses the engine that Image Sage installed." : "Installed."}
              </div>
            ) : download ? (
              <div className="settings-download">
                <div className="settings-line">
                  <CircleNotch className="spin" size={15} />
                  {download.stage === "preparing" ? "Unpacking the engine…" : `Downloading the engine — ${Math.round(download.progress * 100)}%`}
                </div>
                <div className="progress-track"><i style={{ width: `${Math.round(download.progress * 100)}%` }} /></div>
              </div>
            ) : (
              <div className="settings-line">
                <span>The engine (stable-diffusion.cpp, about {(engine.sizeBytes / 1e9).toFixed(1)} GB) is not installed.</span>
                <button className="button primary" onClick={() => void startDownload()}><DownloadSimple size={15} weight="bold" /> Download</button>
              </div>
            )}
          </section>

          <section className="settings-section">
            <div className="settings-heading">
              <h3>Wan 2.2 image-to-video files</h3>
              <div className="settings-heading-actions">
                {view?.imageSageAvailable && (
                  <button className="button secondary small" onClick={() => void copyFromImageSage()} title="Use the Wan 2.2 files and LoRAs that are set in Image Sage">
                    Copy from Image Sage
                  </button>
                )}
                <button className="button secondary small" onClick={() => void findInFolder()} title="Find the four model files in a ComfyUI folder">
                  <MagnifyingGlass size={14} /> Find in Folder…
                </button>
              </div>
            </div>
            {files && MODEL_ROWS.map((row) => (
              <label className="file-row" key={row.key} title={row.help}>
                <span>{row.label}</span>
                <input type="text" value={files[row.key]} spellCheck={false} onChange={(event) => change({ [row.key]: event.target.value })} placeholder="Not set" />
                <button className="button secondary small" onClick={() => void browse(row.key)} aria-label={`Choose the ${row.label}`}><FolderOpen size={15} /></button>
              </label>
            ))}
          </section>

          <section className="settings-section">
            <div className="settings-heading">
              <h3>LoRAs (optional)</h3>
              <button className="button secondary small" onClick={() => setAdding(true)} disabled={!files}><Plus size={14} weight="bold" /> Add LoRA</button>
            </div>
            {files && files.loras.length === 0 && <div className="settings-line">No LoRA. Every clip uses the models as they are.</div>}
            {files && files.loras.length > 0 && (
              <div className="lora-list" ref={loraList}>
                {files.loras.map((pair, index) => (
                  <div className={`lora-row${dragging === index ? " dragging" : ""}`} key={index}>
                    <button
                      className="lora-grip"
                      title="Drag to change the sequence"
                      aria-label="Drag to change the sequence"
                      onPointerDown={(event) => startDrag(event, index)}
                      onPointerMove={moveDrag}
                      onPointerUp={endDrag}
                      onPointerCancel={endDrag}
                    >
                      <DotsSixVertical size={16} weight="bold" />
                    </button>
                    <div className="lora-files">
                      {LORA_SIDES.map((side) => (
                        <div className={`lora-file${pair[side.file] && pair[side.off] ? " off" : ""}`} key={side.file}>
                          <input
                            type="checkbox"
                            checked={!!pair[side.file] && !pair[side.off]}
                            disabled={!pair[side.file]}
                            title={!pair[side.file] ? `This LoRA has no file for the ${side.pass} pass`
                              : pair[side.off] ? `This file is off: the ${side.pass} pass does not use it. Click to use it.`
                              : `The ${side.pass} pass uses this file. Click to turn the file off; it stays in the list.`}
                            aria-label={`Use this LoRA in the ${side.pass} pass`}
                            onChange={(event) => changeLora(index, { [side.off]: !event.target.checked })}
                          />
                          <b>{side.label}</b>
                          <span className={pair[side.file] ? "" : "none"} title={pair[side.file]}>{pair[side.file] ? baseName(pair[side.file]) : "None"}</span>
                          <StrengthInput
                            value={pair[side.strength]}
                            disabled={!pair[side.file]}
                            title="Strength, from 0 to 2: 1 is full strength"
                            label={`The strength of this LoRA in the ${side.pass} pass`}
                            onChange={(value) => changeLora(index, { [side.strength]: value })}
                          />
                        </div>
                      ))}
                    </div>
                    <button className="button secondary small" onClick={() => removeLora(index)} aria-label="Remove this LoRA" title="Remove this LoRA"><Trash size={15} /></button>
                  </div>
                ))}
              </div>
            )}
          </section>

          <section className="settings-section">
            <div className="settings-heading">
              <h3>Negative prompt</h3>
              {view && files && files.negativePrompt !== view.defaultNegativePrompt && (
                <button className="link-button" onClick={() => change({ negativePrompt: view.defaultNegativePrompt })}>Restore the default</button>
              )}
            </div>
            {files && (
              <textarea rows={3} value={files.negativePrompt} spellCheck={false} onChange={(event) => change({ negativePrompt: event.target.value })} />
            )}
          </section>

          {notice && (
            <div className={`settings-line ${notice.error ? "error" : "ok"}`}>
              {notice.error ? <WarningCircle size={16} weight="fill" /> : <CheckCircle size={16} weight="fill" />} {notice.text}
            </div>
          )}
        </div>
        <footer className="dialog-footer">
          <button className="button secondary" onClick={onClose} disabled={!!download}>Cancel</button>
          <button className="button primary" onClick={() => void save()} disabled={!files || saving || !!download}>Save</button>
        </footer>
      </div>
    </div>
    {adding && <LoraDialog folder={loraFolder} onFolder={(folder) => void rememberLoraFolder(folder)} onAdd={addLora} onClose={() => setAdding(false)} />}
    </>
  );
}
