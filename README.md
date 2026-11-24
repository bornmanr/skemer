# Skemer

Skemer is the updater for Veldmuis: a small tray application that reviews and
applies system and Flatpak updates, and browses the official repositories to
install or remove packages, with a Veldmuis profile and theme.

The name is Afrikaans for dusk. The app is not tied to one package manager;
the first release covers the host system and Flatpak, with AUR planned as a
separate, user-run source.

## Status

The scaffold, the system and Flatpak sources, the streaming apply path, the tray
lifecycle, the signed release pipeline, and package browsing with repository
settings are in place. The tray process owns periodic checks, notifications, and
autostart; the window runs in a short-lived `skemer --window` child so closing it
releases WebKitGTK. The design is in [`docs/design.md`](docs/design.md), the
release process in [`docs/release.md`](docs/release.md), and the UI reference is
[`docs/mockup.html`](docs/mockup.html).

## Shape

```text
crates/skemer-core/    models, settings, plan, pacman.conf and hold helpers
crates/skemer-shell/   Tauri tray + window child, CLI routing, D-Bus client
crates/skemer-helper/  privileged D-Bus service and libalpm/pacman apply
src-ui/                TypeScript + Vite frontend
data/                  desktop entry, D-Bus service/policy, polkit policy, icons
docs/                  design and UI reference
tests/fixtures/        shared IPC contract
```

The app follows the same tools as muis: Rust, Tauri 2, WebKitGTK, and a
TypeScript/Vite frontend.

## Build

The Veldmuis/Arch build dependencies include:

```text
rust (stable)
nodejs npm
webkit2gtk-4.1
gtk3
libayatana-appindicator
libalpm
```

Then:

```sh
cargo build --workspace
cargo test --workspace

npm ci --prefix src-ui
npm run build --prefix src-ui
npm test --prefix src-ui
```

The desktop window is created on demand. `skemer --tray` starts in the tray;
`skemer update` is the CLI path and `veldmuis-update` is an alias for it.

## Design boundaries

- The GUI never runs as root.
- The privileged helper is a small D-Bus service with fixed methods.
- libalpm is used for metadata, plans, and hold validation.
- `pacman -Syu` performs the actual system update, so hooks behave exactly as
  they do in pacman.
- System updates are atomic. A failing package can be held for one transaction,
  with explicit confirmation and a visible held state.
- “Reboot suggested”, never “required”.
