# ImageSageVideo repository instructions

## Release memory

- Read `docs/RELEASING.md` before changing installer configuration, preparing a
  release, or publishing.
- Preserve the shared XSage per-user installer contract. Do not switch to
  `perMachine` or Program Files unless the user explicitly reopens that product
  decision.
- Keep `src-tauri/installer.nsi`, the bundled `icons/icon.ico` resource, and the
  Windows GUI-subsystem attribute intact.
- `publish.ps1` builds and checks the installer. A local build is not
  authorization to publish. "Publish" from the user includes committing and
  pushing (the script pushes).
- Use `set-version.ps1` and `publish.ps1`; do not manually create release tags or
  GitHub releases.
- Inside those scripts, call `git`, `gh`, `npm`, and `cargo` only through the
  `Invoke-Native` helper (see `docs/RELEASING.md`).

## Development process

- Stop the dev app with `.\kill-dev.ps1` (tree-kills `imagesagevideo.exe`, which
  also ends the engine, and frees Vite port 14420).
- Do not start the dev app while an installed Image Sage Video is running; the
  single-instance plugin would route the launch to it.
- The job runs in Rust (`src-tauri/src/queue.rs`), not in the webview, so it
  goes on while the window is hidden in the tray. The webview only shows
  `queue-changed` snapshots and sends start, pause, resume and clear.
- A runner may change the job only through `queue::update`, which checks the
  runner's generation. Pause, a new start and clear raise the generation, so
  a runner that was stopped cannot write stale state.
- `src-tauri/src/engine.rs` runs stable-diffusion.cpp's `sd-server.exe` with
  Wan 2.2 image-to-video. Its start flags, job request and pinned engine build
  must stay the same as Image Sage's `src-tauri/src/local.rs`
  (`D:/Code/ImageSage`). It uses Image Sage's installed engine when there is
  one.
- `src-tauri/src/document.rs` only reads `.imagesage` documents (format
  versions 2 and 3). The format belongs to Image Sage; follow its
  `src-tauri/src/document.rs` and `src/editor/document.ts`.
- `src-tauri/src/clip_size.rs` must give the same sizes as Image Sage's
  `src/editor/clipSize.ts`.
- Each layer becomes one video of 1 to 20 clips. Clip 1 starts from the
  layer's picture; each later clip starts from the last picture of the clip
  before (`src-tauri/src/last_frame.rs`). The engine's clips are VP8 key
  pictures only, so that picture is read with the WebP decoder and no video
  decoder is necessary.
- `src-tauri/src/webm.rs` joins a layer's clips without encoding them again,
  and leaves out each later clip's first picture, which repeats the picture
  before it. All clips of one job have the document's shape, so their tracks
  match. The join and its change to the job run in one blocking piece
  (`join_layer` in `queue.rs`), so a pause cannot leave the job between them.
- Files the app keeps for itself (settings, job list) go through
  `src-tauri/src/crypto.rs`. Never write a layer's picture or an unfinished
  clip to the disk. Finished videos are plain WebM files by the user's choice.
- No key or token is ever embedded in the binary.
- Window-control hover follows the WebView2 edge-hover fix
  (`src/components/WindowControls.tsx` and `cursor_position` in `files.rs`).
