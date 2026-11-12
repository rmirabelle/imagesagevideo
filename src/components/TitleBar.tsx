import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { DownloadSimple, GearSix, Info } from "@phosphor-icons/react";
import { WindowControls } from "./WindowControls";

interface Props {
  updateAvailable: boolean;
  onAbout: () => void;
  onSettings: () => void;
}

/**
 * The window's title bar: the app name, the update button when an update is
 * ready, the Settings and About buttons, and the window controls. The
 * minimize button hides the window to the tray.
 */
export function TitleBar({ updateAvailable, onAbout, onSettings }: Props) {
  return (
    <header
      className="titlebar"
      data-tauri-drag-region
      onDoubleClick={() => isTauri() && getCurrentWindow().toggleMaximize()}
    >
      <div className="brand" data-tauri-drag-region>
        <img src="/icon.ico" alt="" />
        <span>Image Sage Video</span>
        {import.meta.env.DEV && <span className="brand-dev">DEV</span>}
      </div>
      {updateAvailable && (
        <button className="titlebar-update-button" onClick={onAbout} title="A new Image Sage Video version is available">
          <DownloadSimple size={14} weight="bold" /> Update available
        </button>
      )}
      <div className="titlebar-spacer" data-tauri-drag-region />
      <div className="titlebar-end">
        <button className="titlebar-button" onClick={onSettings} title="Settings: the engine and the Wan 2.2 model files">
          <GearSix size={16} /> Settings
        </button>
        <button className="titlebar-button" onClick={onAbout} title="About Image Sage Video, and updates" aria-label="About">
          <Info size={16} />
        </button>
      </div>
      <WindowControls />
    </header>
  );
}
