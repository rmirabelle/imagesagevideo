# Image Sage Video

Makes short video clips from every layer of an [Image Sage](https://github.com/rmirabelle/imagesage) document, on your own computer, with Wan 2.2.

You open a `.imagesage` document and choose a folder. For each layer, Image Sage Video makes three 5-second clips. Each clip starts from the layer's picture, and the layer's prompt tells Wan 2.2 what happens. The job runs in the background; the window can hide in the tray.

## Features

- **Three clips for each layer** — small clips (within 832 × 480, in the document's shape), 5 seconds, 16 pictures each second, saved as WebM files.
- **Choose the layers** — every layer is on at the start; turn off the ones you do not want.
- **Combined video** — an option joins all the clips, in order, into one WebM file, with no fade between them. The clips are not encoded again, so the join is fast and loses no quality.
- **Pause and resume** — Pause stops the clip in work and removes the model from the GPU memory. Resume loads the model and makes that clip again from its start.
- **Tray icon** — a minimized window hides to the tray. A dot on the tray icon shows the state: orange while videos are made, green when the job is finished, gray while it is paused. The tooltip shows the progress, and the menu can pause the job and go on with it.
- **A job continues after a restart** — the job list is saved after each clip. If the app closes during a job, the job waits, paused, at the next start.
- **Encrypted app files** — the settings and the job list are encrypted on the disk (XChaCha20-Poly1305). The key is in Windows Credential Manager. A layer's picture and a clip in work stay in memory only. The finished videos are normal WebM files.
- **In-app updates** — Image Sage Video checks GitHub releases and can download and run the new installer.

## File names

`<document>-<layer number>-<layer name>-<1 to 3>.webm`, and `<document>-combined.webm`. A file that exists is never replaced; the new file gets a number at its end.

## Requirements

- Windows 10 or 11 (64-bit).
- An NVIDIA graphics card (24 GB or more of graphics memory works best).
- Wan 2.2 image-to-video's model files: the high-noise and the low-noise model, the UMT5-XXL text encoder, and the Wan 2.1 VAE (for example from a ComfyUI models folder).

The local engine is stable-diffusion.cpp's `sd-server.exe`, the same engine Image Sage uses. When Image Sage has it installed, Image Sage Video uses that copy; if not, **Settings** downloads it (about 0.9 GB). On first use, the model files and LoRAs are copied from Image Sage's settings, when it has them.

## Install

Download the latest `ImageSageVideo_x.y.z_x64-setup.exe` from the [releases page](https://github.com/rmirabelle/imagesagevideo/releases/latest) and run it. Image Sage Video installs for the current user only and does not need administrator rights.

## Development

```powershell
npm install
npm run tauri dev
```

- `cargo test` in `src-tauri` runs the unit tests (the document reader, the clip size rules, the WebM join, the encryption, the job list).
- `.\kill-dev.ps1` stops the dev app, the engine it started, and frees the Vite port (14420).

Releases follow [docs/RELEASING.md](docs/RELEASING.md): `set-version.ps1`, then `publish.ps1`.
