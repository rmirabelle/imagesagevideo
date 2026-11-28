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
- A job's layers can come from many documents and photos. Each layer keeps
  its own file path and clip size (`LayerJob`). `queue_start` starts a job,
  or adds layers to the job that exists; the runner looks for a new clip
  again before it ends, so layers that are added while it runs are made.
  The window keeps the opened files whose layers wait for Start
  (`src/App.tsx`); those layers are not in the job. Start sends only the
  chosen layers; a file's other layers stay in the list until the user
  clears them.
- The list shows the layers in the order the job makes them, from the top
  down. The user drags a row by its grip to move it in its part of the list:
  the layers that wait (`order` in `src/App.tsx`; Start sends them in that
  order) or the job's layers (`queue_move_layer`). A new file's top layer is
  first. Each `LayerJob` has an `id` that does not change when it moves; the
  runner finds its layer by that id, so a move does not raise the generation.
- Cancel Clip (`queue_cancel_clip`) removes the clip in work from its layer and
  starts a new runner; the engine stays loaded. A prompt change in a job
  (`queue_set_prompt`) does not raise the generation; the clips that start
  after it use the new prompt. A done layer's prompt can also change, so
  Again makes the next video with the new prompt.
- Each clip can have its own prompt. Clip 1 follows the layer's prompt
  (`LayerJob.prompt`); a later clip follows its own `Clip.prompt`, or, when
  that is empty, the prompt of the clip before (`LayerJob::clip_prompt`).
  A clip keeps its prompt when it waits again (Retry, Again) and when Cancel
  Clip removes the clip before it. The window keeps the clip prompts of the
  layers that wait (`clipPrompts` in `src/App.tsx`) and shows one text box
  for each clip (`PromptEditor`).
- The engine already encodes each clip at its highest quality
  (`output_compression` 100 is the server's default). A clip's last picture
  cannot be made better without changing the job request, which must stay
  the same as Image Sage's.
- Again (`queue_new_pass`) makes a done layer's video again from the layer's
  picture: its clips wait again, the new video gets a new file name and takes
  the old one's place in the list, and the old video stays in its folder. It
  does not raise the generation; a finished job starts again.
- The list shows each file's name with buttons to show it in its folder,
  rename it and delete it; a done layer has the same for its video.
  `file_rename` and `file_delete` (`queue.rs`) accept only documents, photos
  and videos, give the job a renamed file's new path, and do not raise the
  generation. Delete moves the file to the Recycle Bin. A file that the job
  still needs for a layer's first clip cannot be deleted.
- `src-tauri/src/engine.rs` runs stable-diffusion.cpp's `sd-server.exe` with
  Wan 2.2 image-to-video. Its start flags, job request and pinned engine build
  must stay the same as Image Sage's `src-tauri/src/local.rs`
  (`D:/Code/ImageSage`), with one exception: only this app starts the engine
  with `--offload-to-cpu`, so the end of a clip does not run out of GPU
  memory. It uses Image Sage's installed engine when there is one.
- `src-tauri/src/document.rs` only reads `.imagesage` documents (format
  versions 2 and 3). The format belongs to Image Sage; follow its
  `src-tauri/src/document.rs` and `src/editor/document.ts`.
- `document.rs` also reads a photo (a plain PNG, JPEG or WebP file) as a
  document with one layer, whose file name is `photo` and whose prompt is
  empty. The user writes the prompt in the window.
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
