import { open } from "@tauri-apps/plugin-dialog";
import { FolderOpen, X } from "@phosphor-icons/react";
import { useState } from "react";
import { folderOf, type LoraPair } from "../lib/api";
import { StrengthInput } from "./StrengthInput";

interface Props {
  /** The folder the file browser opens in; empty for the system's choice. */
  folder: string;
  /** The user chose a file in this folder. */
  onFolder: (folder: string) => void;
  onAdd: (pair: LoraPair) => void;
  onClose: () => void;
}

const LORA_FILTER = [{ name: "LoRA files", extensions: ["safetensors", "gguf", "sft"] }];

const SIDES = [
  { file: "highNoise", strength: "highNoiseStrength", label: "High-noise file" },
  { file: "lowNoise", strength: "lowNoiseStrength", label: "Low-noise file" }
] as const;

/**
 * Adds one LoRA to the list: a file for the high-noise model and a file for
 * the low-noise model. One of the two can stay empty.
 */
export function LoraDialog({ folder, onFolder, onAdd, onClose }: Props) {
  const [pair, setPair] = useState<LoraPair>({ highNoise: "", highNoiseStrength: 1, lowNoise: "", lowNoiseStrength: 1, highNoiseOff: false, lowNoiseOff: false });

  const browse = async (side: "highNoise" | "lowNoise") => {
    const picked = await open({ multiple: false, filters: LORA_FILTER, defaultPath: folder || undefined });
    if (typeof picked !== "string") return;
    setPair((current) => ({ ...current, [side]: picked }));
    onFolder(folderOf(picked));
  };

  const ready = pair.highNoise.trim() !== "" || pair.lowNoise.trim() !== "";

  return (
    <div className="dialog-overlay" role="presentation" onPointerDown={onClose}>
      <div className="dialog lora-dialog" role="dialog" aria-modal="true" aria-labelledby="lora-title" onPointerDown={(event) => event.stopPropagation()}>
        <header className="dialog-header">
          <h2 id="lora-title">Add a LoRA</h2>
          <button className="dialog-close" onClick={onClose} aria-label="Close"><X size={18} /></button>
        </header>
        <div className="dialog-body">
          <p className="lora-help">A Wan 2.2 LoRA has a file for each model. Choose one file or the two files.</p>
          {SIDES.map((side) => (
            <label className="file-row with-strength" key={side.file}>
              <span>{side.label}</span>
              <input
                type="text"
                value={pair[side.file]}
                spellCheck={false}
                placeholder="None"
                onChange={(event) => setPair((current) => ({ ...current, [side.file]: event.target.value }))}
              />
              <StrengthInput
                value={pair[side.strength]}
                title="Strength, from 0 to 2: 1 is full strength"
                label={`The strength of the ${side.label}`}
                onChange={(value) => setPair((current) => ({ ...current, [side.strength]: value }))}
              />
              <button className="button secondary small" onClick={() => void browse(side.file)} aria-label={`Choose the ${side.label}`}><FolderOpen size={15} /></button>
            </label>
          ))}
        </div>
        <footer className="dialog-footer">
          <button className="button secondary" onClick={onClose}>Cancel</button>
          <button className="button primary" disabled={!ready} onClick={() => onAdd({ ...pair, highNoise: pair.highNoise.trim(), lowNoise: pair.lowNoise.trim() })}>Add</button>
        </footer>
      </div>
    </div>
  );
}
