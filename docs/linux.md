# Linux development

This port is based on upstream commit `9ab00933df48ba44cbbd8e180259e48525a930e3` (v0.9.0). It keeps the Vue interface and the Rust provider code shared with macOS and Windows. Linux support is experimental: a debug build has run on Pop!_OS 24.04 with GNOME 46 and X11, including an isolated GNOME/Xvfb session. Other desktops and Wayland have not been qualified.

## Build and run

Install Node.js 24 or newer, pnpm 11.25.0, Rust 1.90 or newer and the [Tauri Linux prerequisites](https://v2.tauri.app/start/prerequisites/), including the GTK 3, WebKitGTK 4.1 and Ayatana AppIndicator development libraries. Then run:

```sh
export CARGO_BUILD_JOBS="$(nproc)"
pnpm install --frozen-lockfile
pnpm tauri dev
```

`pnpm tauri dev` starts Vite and the native application. For a release build, use `pnpm tauri build`; Linux packaging has not yet been verified. A debug binary built with Cargo alone expects the Vite server configured in `src-tauri/tauri.conf.json` to be running.

## Desktop and accounts

For tray access, the desktop needs a system-tray implementation. GNOME may need an AppIndicator extension. On Linux, Tauri does not emit its tray click events; use the tray menu's **Open Margin** item to open the window. Launching a second instance also opens the existing window. Without the optional GNOME bridge below, anchoring to the tray is not guaranteed. See [Tauri's tray event documentation](https://docs.rs/tauri/latest/tauri/tray/enum.TrayIconEvent.html).

### GNOME Shell 46 bridge

The optional bridge supplies a panel indicator and positions Margin below it. It has been exercised with GNOME Shell 46 on X11 in Xvfb; Wayland, other Shell versions and mixed-DPI displays are not yet qualified. Install it explicitly from a checkout:

```sh
gnome-extensions pack linux/gnome-shell/margin@bepcyc.github.io --out-dir /tmp
gnome-extensions install /tmp/margin@bepcyc.github.io.shell-extension.zip
```

On GNOME 46, installation can fail with `Can’t recursively copy directory` when the cache and extension directories are on different filesystems. If that happens, copy the extension files from the checkout instead:

```sh
extension_dir="${XDG_DATA_HOME:-$HOME/.local/share}/gnome-shell/extensions/margin@bepcyc.github.io"
install -d -m 755 "$extension_dir"
install -m 644 linux/gnome-shell/margin@bepcyc.github.io/metadata.json \
  linux/gnome-shell/margin@bepcyc.github.io/extension.js "$extension_dir/"
```

Sign out and back in so GNOME Shell discovers the newly installed extension, then enable it:

```sh
gnome-extensions enable margin@bepcyc.github.io
```

Restart Margin after enabling or disabling the extension. If the extension was absent when Margin started, the existing StatusNotifier tray is used until restart. If a connection fails after registration begins, Margin opens a recovery notice rather than creating a second tray icon; close and restart it to retry. A session-bus failure with a live but disconnected Shell may leave an old indicator across restarts.

The bridge is intended for a trusted local desktop session. It binds requests to a D-Bus connection and associates a window by PID and window class; these checks do not authenticate Margin against other processes of the same user. The icon and its tooltip are sent over the session bus, but credentials are not.

To remove the bridge, close Margin, then run:

```sh
gnome-extensions disable margin@bepcyc.github.io
gnome-extensions uninstall margin@bepcyc.github.io
```

Margin discovers OAuth sign-ins from `~/.claude/.credentials.json`, `~/.codex/auth.json`, matching home dotfolders and the `CLAUDE_CONFIG_DIR` or `CODEX_HOME` environment variables. Other folders can be added in Settings. A Codex account stored only in a system credential store is not discovered by the file reader. An API key alone does not provide OAuth usage data. Tokens stay in the Rust backend; do not attach credentials, account details or raw provider responses to reports.

In one local run, a Codex account produced a fresh usage snapshot. Other discovered accounts were signed out. This does not verify all provider configurations or desktop environments.

## Updates and libc

This Linux build does not check or install upstream updates. Rebuild or update it through the source or package used to install it; no Linux package or updater is published by this port yet.

The tested build uses glibc and system-compatible GTK/WebKit libraries. [Tauri documents Alpine prerequisites](https://v2.tauri.app/start/prerequisites/) for musl systems, but a musl build of Margin has not been tested. Static musl alone does not remove the GTK and WebKit dependencies; a separate Alpine build and runtime check are needed before distributing one.
