# src-rs — Prizrak-Box desktop shell on Rust + Slint (TEST)

Native replacement for the Wails v3 shell (`src-wails`) and the Electron shell
(`src-electron`): **no web engine at all**. The UI is written in
[Slint](https://slint.dev) (`ui/*.slint`), the logic in Rust (`src/`), and it
drives the very same backend — `px` (`src-go`) and `px-service`
(`src-service`) — over the very same contracts. Why Slint, and the
alternatives that were considered, are in
[`doc/rust-slint-migration-analysis.md`](../doc/rust-slint-migration-analysis.md).

## Layout

| Path | What it is | Ported from |
|---|---|---|
| `ui/theme.slint` | design tokens, translator global | `src/styles/tokens.css`, `useAppTheme.ts` |
| `ui/components.slint` | buttons, pills, switches, inputs, modals, notices, select, tooltip… | `src/components/ui/*`, `src/styles/ui.css` |
| `ui/glass.slint` | backdrop-blur emulation (pre-blurred background slice) | CSS `backdrop-filter` |
| `ui/shell.slint` | sidebar, header proxy selector, title-bar buttons, global dialogs, toasts | `App.vue`, `MyNav/MyRule/MyProxy/MyBottom/Off/Language`, `MySearch`, `MyTitleBar` |
| `ui/home.slint` | Home page, add-profile dialog | `views/Home.vue`, `components/home/*` |
| `ui/proxies.slint` | Proxies page | `views/Proxies.vue` |
| `ui/profiles.slint` | Profiles page and its dialogs | `views/Profiles.vue` |
| `ui/settings.slint`, `ui/setting_page.slint` | Settings (app/core), DNS editor, hotkey, age keys, dashboards | `views/Setting.vue`, `components/setting/*`, `views/setting/*` |
| `ui/rules.slint` | Rule tab (now / templates / providers / ignore) | `views/rule/*` |
| `ui/conn.slint` | Connections (list / sankey topology / processes) + Log tab | `ConnectionTab.vue`, `ConnectionTopology.vue`, `LogTab.vue` |
| `ui/theme_dialog.slint` | theme dialog | `components/menu/Skin.vue` |
| `src/core.rs` | px spawn/supervise, `/pxStore` + `/pxAlive` callback server | `src-wails/services/core.go` |
| `src/tun.rs` | px-service IPC, elevation, service state | `src-wails/services/tun.go`, `elevate.go`, `svcstate_*.go` |
| `src/api.rs`, `src/ws.rs` | px REST + WebSocket client | `src/api/*`, `src/util/ws.ts` |
| `src/store.rs`, `src/migrate.rs` | settings file + one-time migration | electron-store / Wails kvstore, webview localStorage |
| `src/theme.rs` | background loading, accent from image, blur | `src/util/theme.ts` |
| `src/tray.rs`, `src/hotkey.rs`, `src/autostart.rs`, `src/fileicon.rs`, `src/deeplink.rs`, `src/single_instance.rs`, `src/notify.rs` | OS integration | `src-wails/{tray.go,hotkeys.go}`, `services/*` |
| `src/app.rs`, `src/pages/*` | controllers | the Vue components' `<script>` blocks |

Translations are the Vue app's `src/locales/*.yaml`, embedded at build time —
one source of truth for every shell.

## Compatibility with the other shells

* data dir `~/Prizrak-Box-V3` (or the `home.path` override), px started as
  `px -addr=127.0.0.1:<cb> -home=<dir>` exactly like Wails;
* settings in `<home>/px-electron.db/config.json` in the electron-store format
  (values are the pinia JSON strings), shared both ways with Electron/Wails;
* shell flags in `<UserConfigDir>/prizrak-box/` (`start-minimized`,
  `tun-desired`, `dark-bg`, `home.path`, `shell.log`);
* TUN stack: the first launch of this shell sets **Mips** for everyone
  (also over a stack chosen in Electron/Wails), once — flag `rs-stack-default`;
  after that the user's choice in Settings → Core is kept;
* `px-service` IPC (unix socket / `\\.\pipe\prizrak-box-service`) unchanged;
* autostart: Windows task "Prizrak-Box Startup", macOS LaunchAgent, Linux
  `~/.config/autostart/prizrak-box.desktop`;
* deep links `prizrak-box://install-config?url=…&name=…`;
* the last remote background (random themes: anime, games, …) is kept in
  `<UserConfigDir>/prizrak-box/bg-cache.{key,img}`, so a restart shows the
  picture that was picked instead of fetching a new random one; Electron's
  `px-bg-cache.json` is read once on upgrade;
* packages: same bundle id / MSI product chain (supersedes Electron and the
  Wails MSI) / Linux package name `prizrak-box`, so installing over any older
  build (1.0.20 Electron, 1.0.21 alphas/betas on Wails) upgrades in place.
  Things that only existed inside the old web engines (custom dashboards,
  early-alpha settings kept in `localStorage`) are imported once by
  `src/migrate.rs`.

## Window chrome per OS

* **macOS** — native traffic lights over a transparent full-size title bar,
  plus one button ("hide to tray"), like the Wails/Electron builds.
* **Windows / Linux** — frameless window with four buttons: hide to tray,
  minimise, maximise/restore, close (quits); drag and resize are done through
  winit.

## Build & run

```bash
# backend binaries
(cd src-go && CGO_ENABLED=0 go build -tags=with_gvisor -trimpath -o px .)
(cd src-service && CGO_ENABLED=0 go build -o px-service .)

# the shell (needs Rust ≥ 1.85)
cd src-rs
cargo run --release
```

Linux build deps: `libgtk-3-dev libayatana-appindicator3-dev libfontconfig1-dev libxkbcommon-dev pkg-config`.

Environment overrides (same as the Wails shell): `PRIZRAK_PX_BIN`,
`PRIZRAK_PX_SERVICE_BIN`, `PRIZRAK_HOME`.

### Visual checks without a display session

`PRIZRAK_SNAPSHOT_DIR=/tmp/shots PRIZRAK_SNAPSHOT_SEQ="Home,Proxies,Profiles,Setting:core,Setting:log,Dialog:theme" cargo run`
renders each step to a PNG (via `Window::take_snapshot`) and quits.

## Packaging

* macOS: `packaging/macos/build-app.sh <binary> <version>` → `Prizrak-Box.app`,
  then `node src-wails/build/darwin/create-macos-dmg.mjs <arch> <out.dmg>`
  (the Wails DMG builder);
* Windows: stage `Prizrak-Box.exe` + `resources/px.exe` + `resources/px-service.exe`
  into `out/Prizrak-Box-win32-<arch>` and run `BUILD_TYPE=wails node installer/build-msi.js`
  (the Wails WiX MSI);
* Linux: `nfpm pkg -f src-rs/packaging/linux/nfpm.yaml`.

CI: `release.yml` jobs `build_{windows,macos,linux}_rs` (artifacts
`*-rs.{msi,zip,dmg,deb,rpm,tar.zst}`, listed as **Rust Slint TEST** in the
release notes) and `rs-build.yml` for pull requests.
