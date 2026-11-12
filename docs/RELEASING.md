# ImageSageVideo release playbook

This is the durable source of truth for building, testing, and publishing
ImageSageVideo. Do not publish directly from memory or bypass `publish.ps1`.

## Windows installer contract

ImageSageVideo follows the shared XSage installer protocol used by Image Sage,
DB Sage and the other XSage desktop applications:

- Installation is **per-user** (`currentUser`), under the user's local application
  data. Do not change it to `perMachine` or Program Files without an explicit new
  product decision and a full installer migration plan.
- `src-tauri/installer.nsi` is the shared custom XSage NSIS template.
- `src-tauri/icons/icon.ico` must be both the bundle icon and a bundled resource.
- The installer copies the icon to
  `C:\Users\Public\${PRODUCTNAME}\icon.ico`, then assigns that explicit path and
  the AppUserModelId to Start Menu and optional desktop shortcuts. Explorer does
  not reliably expand an environment-variable icon path in shortcut metadata.
- Release builds use the Windows GUI PE subsystem. The release executable must
  never open a console window.
- The installer template is copied from IconSage
  (`D:/Code/IconSage/src-tauri/installer.nsi`) with one change, the same as
  Image Sage's and DB Sage's: in passive mode (`/P`), `PageLeaveReinstall` takes
  the first choice (uninstall the old version, keep app data) without showing
  the page.
- In-app updates exit ImageSageVideo, then run setup with `/P /R`: no setup
  pages, and the app starts again when the install finishes. A job that ran is
  kept in the job list and waits, paused, in the new version.
- ImageSageVideo has a tray icon. A minimized window hides to the tray. Closing
  the window quits the app; while a job runs, the app asks first.
- No key or token is ever embedded in the binary. The key that encrypts the
  app's own files is made on the user's computer and kept in Windows Credential
  Manager.

`publish.ps1` validates these rules before it creates a tag and validates the
generated executable and NSIS script again after the release build.

## Release workflow

1. Stop the Tauri dev process (`.\kill-dev.ps1`).
2. Synchronize the intended semantic version:

   ```powershell
   .\set-version.ps1 0.2.0
   ```

3. Run the tests (`cargo test` in `src-tauri`).
4. Commit the source changes. Publishing requires a clean work tree and the
   public `rmirabelle/imagesagevideo` origin.
5. Confirm GitHub CLI authentication, then publish:

   ```powershell
   gh auth status
   .\publish.ps1
   ```

   The script builds the installer and checks it automatically. It also pushes
   the commit and the version tag, so when the user says "publish", that
   includes the push.
6. Confirm the printed release URL.

If the user asks for a manual installer check, install the NSIS bundle from
`src-tauri\target\release\bundle\nsis` with a **GUI** install (not `/S`) and
check the window, the tray icon, and the shortcuts' icon.

The script creates and verifies the new public release first, then deletes every
older release and its tag. If verification fails before cleanup, older releases
are intentionally preserved.

## Local engine

The engine is stable-diffusion.cpp's `sd-server.exe` (Windows CUDA 12 build)
and its CUDA runtime, about 0.9 GB, from the upstream releases at
github.com/leejet/stable-diffusion.cpp. `src-tauri/src/engine.rs` pins the build
(`ENGINE_BUILD`) and each zip's URL, size and SHA-256. These must be the same
as in Image Sage's `src-tauri/src/local.rs`, because ImageSageVideo runs Image
Sage's installed copy (`%LOCALAPPDATA%\com.robmirabelle.imagesage\engines\<build>`)
when there is one. If not, it unpacks its own copy into
`%LOCALAPPDATA%\com.robmirabelle.imagesagevideo\engines\<build>`.

To move to a newer build, change Image Sage first, then copy `ENGINE_BUILD`
and `ENGINE_ZIPS` here, and before release make one layer's video of two clips
or more. That test also shows that the engine's clips are still VP8 key
pictures, which `last_frame.rs` needs. The model files are the user's own; the app never downloads
them.

## Windows PowerShell 5.1 note

`set-version.ps1` and `publish.ps1` run under Windows PowerShell 5.1 with
`$ErrorActionPreference = "Stop"`. In 5.1, a native command's stderr output
becomes a `NativeCommandError` record whenever stderr is redirected or the host
is not a plain console. Under `Stop`, the first such line aborts the script even
though the command succeeded. Both scripts therefore call every native command
(`git`, `gh`, `npm`, `cargo`) through the `Invoke-Native` helper, which
temporarily sets the preference to `Continue` and fails on the exit code
instead. Keep that pattern when adding new native calls.
