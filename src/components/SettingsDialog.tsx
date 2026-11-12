import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { CheckCircle, CircleNotch, DownloadSimple, FolderOpen, MagnifyingGlass, WarningCircle, X } from "@phosphor-icons/react";
import { useEffect, useState } from "react";
import {
  engineDownload,
  engineStatus,
  findModels,
  settingsFromImageSage,
  settingsGet,
  settingsSet,
  type EngineStatus,
  type ModelFiles,
  type SettingsView
} from "../lib/api";

interface Props {
  onClose: () => void;
}

type FileKey = "highNoiseModel" | "diffusionModel" | "textEncoder" | "vae" | "highNoiseLora" | "lora";

const MODEL_ROWS: { key: FileKey; label: string; help: string }[] = [
  { key: "highNoiseModel", label: "High-noise model", help: "Wan 2.2 image-to-video, high noise. It starts each clip." },
  { key: "diffusionModel", label: "Low-noise model", help: "Wan 2.2 image-to-video, low noise. It finishes each clip." },
  { key: "textEncoder", label: "Text encoder", help: "The UMT5-XXL text encoder." },
  { key: "vae", label: "VAE", help: "The Wan 2.1 VAE." }
];

const LORA_ROWS: { key: FileKey; strength: "highNoiseLoraStrength" | "loraStrength"; label: string }[] = [
  { key: "highNoiseLora", strength: "highNoiseLoraStrength", label: "High-noise LoRA" },
  { key: "lora", strength: "loraStrength", label: "Low-noise LoRA" }
];

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

  useEffect(() => {
    void settingsGet().then((loaded) => {
      setView(loaded);
      setFiles(loaded.settings.files);
    });
    void engineStatus().then(setEngine);
  }, []);

  useEffect(() => {
    const unlisten = listen<{ stage: string; progress: number }>("engine-download", (event) => setDownload(event.payload));
    return () => { void unlisten.then((stop) => stop()); };
  }, []);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !download) onClose();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onClose, download]);

  const change = (patch: Partial<ModelFiles>) => setFiles((current) => (current ? { ...current, ...patch } : current));

  const browse = async (key: FileKey) => {
    const picked = await open({ multiple: false, filters: MODEL_FILTER, defaultPath: files?.[key] || undefined });
    if (typeof picked === "string") change({ [key]: picked });
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
            <h3>LoRAs (optional)</h3>
            {files && LORA_ROWS.map((row) => (
              <label className="file-row with-strength" key={row.key}>
                <span>{row.label}</span>
                <input type="text" value={files[row.key]} spellCheck={false} onChange={(event) => change({ [row.key]: event.target.value })} placeholder="None" />
                <input
                  type="number"
                  min={0}
                  max={2}
                  step={0.05}
                  value={files[row.strength]}
                  title="Strength: 1 is full strength, 0 turns the LoRA off"
                  onChange={(event) => change({ [row.strength]: Number.isFinite(event.target.valueAsNumber) ? event.target.valueAsNumber : 0 })}
                />
                <button className="button secondary small" onClick={() => void browse(row.key)} aria-label={`Choose the ${row.label}`}><FolderOpen size={15} /></button>
              </label>
            ))}
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
  );
}
