# Linux development

This port is based on upstream commit `9ab00933df48ba44cbbd8e180259e48525a930e3` (v0.9.0). It keeps the Vue interface and the Rust provider code shared with macOS and Windows. Linux support is experimental: a debug build has run on Pop!_OS 24.04, GNOME 46 and X11, but other desktops and Wayland have not been qualified.

## Build and run

Install Node.js 24 or newer, pnpm 11.25.0, Rust 1.90 or newer and the [Tauri Linux prerequisites](https://v2.tauri.app/start/prerequisites/), including the GTK 3, WebKitGTK 4.1 and Ayatana AppIndicator development libraries. Then run:

```sh
export CARGO_BUILD_JOBS="$(nproc)"
pnpm install --frozen-lockfile
pnpm tauri dev
```

`pnpm tauri dev` starts Vite and the native application. For a release build, use `pnpm tauri build`; Linux packaging has not yet been verified. A debug binary built with Cargo alone expects the Vite server configured in `src-tauri/tauri.conf.json` to be running.

## Desktop and accounts

For tray access, the desktop needs a system-tray implementation. GNOME may need an AppIndicator extension. On Linux, Tauri does not emit its tray click events; use the tray menu's **Open Margin** item to open the window. Launching a second instance also opens the existing window. Window positioning depends on the desktop and display server; anchoring to the tray is not guaranteed, especially on Wayland. See [Tauri's tray event documentation](https://docs.rs/tauri/latest/tauri/tray/enum.TrayIconEvent.html).

Margin discovers OAuth sign-ins from `~/.claude/.credentials.json`, `~/.codex/auth.json`, matching home dotfolders and the `CLAUDE_CONFIG_DIR` or `CODEX_HOME` environment variables. Other folders can be added in Settings. A Codex account stored only in a system credential store is not discovered by the file reader. An API key alone does not provide OAuth usage data. Tokens stay in the Rust backend; do not attach credentials, account details or raw provider responses to reports.

In one local run, a Codex account produced a fresh usage snapshot. Other discovered accounts were signed out. This does not verify all provider configurations or desktop environments.

## Updates and libc

This Linux build does not check or install upstream updates. Rebuild or update it through the source or package used to install it; no Linux package or updater is published by this port yet.

The tested build uses glibc and system-compatible GTK/WebKit libraries. [Tauri documents Alpine prerequisites](https://v2.tauri.app/start/prerequisites/) for musl systems, but a musl build of Margin has not been tested. Static musl alone does not remove the GTK and WebKit dependencies; a separate Alpine build and runtime check are needed before distributing one.
