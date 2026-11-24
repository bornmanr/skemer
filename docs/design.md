# Skemer Design

Status: agreed design for the first release  
UI reference: [`mockup.html`](mockup.html) · [PNG](mockup.png)

## 1. What Skemer Is

Skemer is the Veldmuis updater: a small tray application that reviews and
applies system and Flatpak updates. It is built like muis, with Rust, Tauri 2,
WebKitGTK, and a TypeScript/Vite frontend, but it is not tied to Veldmuis.
Veldmuis ships the branded profile and theme; other Arch systems can use the
generic profile.

The first release covers:

- the host system (`pacman -Syu`);
- Flatpak system updates;
- Flatpak user updates;
- installing and removing official packages, with repository settings
  (multilib for Steam).

AUR is deliberately deferred. It will be a separate, user-run source with a
PTY-backed worker and a persistent user-supported warning. See §13 and §14 for
package management.

## 2. Product Behavior

- Tray application with autostart.
- Default check interval: 6 hours (configurable: 1/3/6/12/24 hours, or never).
- Desktop notifications on by default.
- Close to tray by default; tray “Quit” exits.
- The main window is created on demand. It runs in a short-lived
  `skemer --window` child process; closing it exits that process and releases
  WebKitGTK. The tray and periodic checker live in the Rust parent process.
- Window: 960 × 640 default, 760 × 520 minimum, not maximized.
- “Reboot suggested”, never “required”. Amber chip, not red.

## 3. Sources

### System

- **libalpm** (official `alpm` crate 5.x) for metadata, plans, transaction
  preparation, conflict detection, and hold validation.
- **`pacman -Syu`** for the actual apply, so pacman hooks and transaction
  behavior are exactly pacman’s.
- System updates are atomic. No per-package official updates.
- A failing package can be held for one transaction, but only when libalpm can
  prepare the transaction with that package ignored.
- The hold is explicit, visible, and per-transaction. It is never persisted as
  `IgnorePkg`, and it is never a silent skip.
- After a successful hold-and-retry, the held package is shown as “Held” and a
  “Retry held” action is available.

### Flatpak

Two sources:

- **Flatpak system** — via the privileged D-Bus helper, polkit-authorized.
- **Flatpak user** — runs as the desktop user, no prompt.

One Flatpak tab with two groups. Runtimes are included with the apps. “Update
all” applies system, then Flatpak system under the same polkit authorization,
then Flatpak user.

### AUR

Deferred. No AUR tab, no AUR code path, and no AUR privilege path in v1.

## 4. Privilege Model

```text
skemer --tray (user)
  ├─ tray, periodic checker, notifications (Rust)
  ├─ skemer --window (user, on demand)
  │    ├─ WebKitGTK window; exits when closed
  │    └─ Flatpak user updates (user)
  └─ D-Bus → org.veldmuislinux.skemer
       └─ skemer-helper (root, systemd/D-Bus activated)
            ├─ libalpm check/prepare
            ├─ pacman -Syu
            ├─ flatpak system update
            ├─ veldmuis-refresh-arch-mirrors / reflector
            ├─ pacman -Syu --needed <pkgs>   (planned, §13)
            ├─ pacman -Rns <pkgs>            (planned, §13)
            └─ managed pacman.conf writes    (planned, §14)
```

- The GUI never runs as root.
- The helper exposes fixed methods only. No command strings, no arbitrary
  paths, no shell interpolation.
- `Check` is allowed for active local users; it is read-only.
- `Prepare` validates holds with libalpm; it is read-only.
- `Apply` requires polkit `auth_admin_keep`.
- `RefreshMirrors` requires polkit `auth_admin_keep`.
- `Cancel` is allowed for the caller that started the apply.
- One transaction at a time. The helper releases the pacman DB lock on exit.
- AUR helpers are never run by the helper.
- Logs go to the journal; the UI shows the streamed log.
- The tray parent and window child use small signals: SIGHUP reloads settings,
  SIGUSR1 refreshes the tray state, SIGUSR2 focuses an existing window, and
  SIGTERM closes the whole app when close-to-tray is off.

The D-Bus name is `org.veldmuislinux.skemer`; the polkit action is
`org.veldmuislinux.skemer.apply`.

## 5. Hold And Retry

1. `pacman -Syu` fails.
2. The helper returns the error and likely package candidates.
3. The UI stops; it does not silently continue.
4. If libalpm can prepare the transaction with one candidate ignored, the UI
   offers “Hold `<pkg>` and retry the rest”.
5. The user confirms; the hold is visible in the summary and the package list.
6. The helper runs `pacman -Syu --ignore <pkg>`.
7. If the hold fails, the UI shows the new error and does not loop.
8. The held package is listed as “Held”; “Retry held” is available later.
9. If the held package is the one the user actually cares about, the UI says
   plainly that it will not be updated.

No automatic holds. No permanent ignores. No silent exclusions.

## 6. Reboot

“Reboot suggested” is triggered by:

- kernel packages (`linux`, `linux-lts`, `linux-zen`, `linux-hardened`,
  `linux-rt`);
- `linux-firmware`;
- `systemd`;
- `mkinitcpio`/initramfs changes;
- `mesa`, NVIDIA drivers, DKMS;
- the running kernel’s module directory disappearing.

Graphics-only changes can say “Restart session suggested” instead of a full
reboot.

## 7. Identity And Branding

- Binary: `skemer`
- Display name: `Skemer`
- Tauri identifier: `org.veldmuislinux.skemer`
- Veldmuis package: `veldmuis-skemer`
- Desktop entry: `skemer.desktop`
- D-Bus: `org.veldmuislinux.skemer`
- Polkit: `org.veldmuislinux.skemer.apply`
- Helper: `skemer-helper`
- Polkit for package management (planned):
  `org.veldmuislinux.skemer.manage-packages` and
  `org.veldmuislinux.skemer.manage-repositories`

The core is distro-agnostic. A `DistroProfile` selects:

- display name, desktop entry, icon;
- theme/colors;
- repository labels;
- mirror refresh command (`veldmuis-refresh-arch-mirrors` vs `reflector` or
  none);
- default sources.

The Veldmuis package supplies the Veldmuis profile and theme. Upstream ships a
neutral default. The layout is the same; branding is data, not hard-coded.

## 8. CLI

Skemer replaces the bash `veldmuis-update` wrapper.

- `skemer` — open the updater.
- `skemer --tray` — start in the tray.
- `skemer check [--json]` — show the plan.
- `skemer update [--hold PKG]` — apply updates; requires root.
- `veldmuis-update` — argv[0] alias to `skemer update`.

The CLI is the root/recovery path. The GUI is the polkit path. `skemer check`
asks the helper for a read-only plan; if the helper is unavailable it says so
and suggests starting `skemer-helper`.

## 9. Delivery

- Separate repository: `Skemer`.
- Separate Skemer release key; signed tarball; Veldmuis pin; release watcher.
- `veldmuis-skemer` packages the signed tarball like `veldmuis-muis`.
- libalpm ABI: the package depends on the exact `libalpm.so` soname. Skemer and
  libalpm upgrade atomically in `pacman -Syu`. An ABI watcher forces a new
  Skemer build when the soname changes.
- If the new Skemer build is not available, the dependency blocks the libalpm
  upgrade rather than leaving a broken Skemer.

## 10. Testing

- Rust unit tests for plan totals, hold candidates, reboot detection, Flatpak
  parsing, and settings.
- Fake `pacman`/`flatpak` binaries for apply/log parsing.
- D-Bus service tests on a private bus with a test policy.
- UI Vitest tests and Playwright interaction tests.
- One VM release gate for real system + Flatpak + polkit.

## 11. Phases

1. Scaffold: workspace, models, settings, plan helpers, source traits, tray
   skeleton, D-Bus skeleton, UI shell, docs, CI.
2. libalpm system source: check, prepare, hold validation.
3. D-Bus helper: polkit, pacman apply, log streaming, cancel.
4. Flatpak system/user: check, apply, grouping.
5. Tray: periodic checks, notifications, autostart, window lifecycle.
6. Packaging and release: Veldmuis package, keyring, release watcher, ABI
   watcher.
7. AUR: deferred follow-up.
8. Package management (§13): official-repository search, install, remove,
   transaction preview, and repository settings (multilib).

## 12. Open Implementation Details

- The fixed D-Bus methods are `Check`, `Prepare`, `Apply`, `Cancel`, and
  `RefreshMirrors`. `Apply` emits `Progress` and `LogLine` signals while it
  runs, and `Cancel` sends SIGINT to the pacman process group. Only the user
  that started the update may cancel it.
- Flatpak runtime version display when the remote does not expose a version;
  the branch is shown in the summary today.
- Theme loader format (CSS variables + a small theme manifest).
- Log retention and copy-log behavior.
- Tray menu copy and icon states.
- Package dependency list for `veldmuis-skemer`.
- Release watcher and ABI watcher workflow.

## 13. Package Management

Status: implemented. It reuses the helper, the transaction lock, and the polkit
style but adds no new update behavior.

### 13.1 Why Skemer and not PackageKit/Discover

Veldmuis ships Plasma without Discover or PackageKit, so a fresh install has no
graphical way to add software. The two options are:

- ship `discover` + `packagekit`, which is nearly free but introduces a second
  transaction path layered over pacman and splits updates across two apps;
- extend Skemer, which already has libalpm, the root helper, polkit, the
  single-transaction lock, and streamed logs.

Skemer is the better owner: one privileged surface, the same hooks, the same
ignore/hold semantics, and one branded app. Search stays local; the only
network access is pacman's own database sync.

### 13.2 Scope

- Search the enabled sync databases (core, extra, multilib, and the Veldmuis
  repositories). On a current Veldmuis install that is about 15,800 packages.
- Install and remove official packages only.
- `openai-codex` is `extra/openai-codex`; Steam is `multilib/steam`.
- No AUR, no Flatpak app discovery, no downgrades, no testing repositories, no
  custom repositories, no screenshots, ratings, or categories.
- Search-first. There is no curated home page or featured list.

### 13.3 Helper API

Read-only calls are allowed for active local users. Writes require polkit.
Every call takes JSON, returns JSON, and is a fixed method; the client never
passes flags, paths, or shell text.

```text
SearchPackages      read-only   query, optional repo, installed-only, limit
PackageInfo         read-only   one exact package name
PreviewPackages     read-only   action + names, validates with libalpm
ApplyPackages       polkit      action + names, streams Progress and LogLine
RepositoryStatus    read-only   effective repositories and provenance
SetRepository       polkit      allowed repo name + enabled flag
```

- `SearchPackages` returns name, repo, version, installed version (or none),
  download and installed size, summary, and groups. Default limit 50, maximum
  100. No pagination in v1.
- `PackageInfo` returns depends, optional depends, conflicts, replaces,
  provides, groups, licenses, URL, packager, build date, and, when installed,
  the reverse dependencies from the local database.
- `PreviewPackages` runs a libalpm transaction with the requested packages and
  returns `adds`, `removes`, `total_download_bytes`,
  `total_installed_bytes`, `pending_system_updates`, `reboot_suggested`, and
  `warnings`. This is the same read-only preparation pattern as `Prepare`.
- `ApplyPackages` uses the update transaction lock. `Cancel` from the user that
  started it sends SIGINT to the process group, exactly as it does for updates.

Validation before anything is passed to pacman:

- names must match an exact sync or local package name, or an exact group name
  in an enabled repository;
- ASCII `[a-z0-9@._+-]`, at most 128 characters, no `/`, whitespace, glob, or
  leading `-`;
- at most 64 names per request;
- a name that resolves to several providers is rejected unless the UI sent the
  concrete provider name.

### 13.4 Transaction Rules

1. Install is one transaction: `pacman -Syu --needed --noconfirm <pkgs>` with
   `LC_ALL=C` and `--color never`. Never a bare `pacman -S`.
2. Folding the full sync in is deliberate. Installing from an unsynced or stale
   setup is the classic partial upgrade; `-Syu` makes the install atomic with
   the rest of the system. The confirmation sheet shows how many pending updates
   and how much download the install drags in.
3. `--needed` avoids reinstalling packages that are already current.
4. Replacements reported by libalpm appear in the preview and are applied by
   `-Syu`; they are never silent.
5. Remove is `pacman -Rns --noconfirm <pkgs>`, and only after the preview lists
   the full removal closure, including dependencies that become unneeded. No
   orphan removal happens unless the user explicitly confirmed that list.
6. Group targets are accepted by name. The helper resolves the group with
   libalpm for the preview, then pacman installs the group itself, so the
   transaction matches pacman exactly. Member deselection is a follow-up; v1
   installs the whole group.
7. Provider and virtual-package choices are explicit. The helper never guesses.
8. Optional dependencies are shown but never installed automatically. Each one
   can be added to the request as a normal package.
9. One transaction at a time, shared with updates. A failing install returns the
   error and the log tail; there is no hold-and-retry loop on installs.
10. Reboot rules from §6 apply after a successful install or removal.

Refresh of the databases is not exposed as its own action. `-Sy` alone is only
safe because Skemer never installs with `-S`; the install transaction always
does the full sync.

### 13.5 UI

- A top-level switch `Updates | Browse` above the existing source tabs. The
  `System` and `Flatpak` tabs and their counts stay inside `Updates`; `Browse`
  has no counts and no update state.
- Search box with a short debounce. Results show name, repository chip (the
  existing `multilib` chip style), version, and an installed marker. Ranking is
  exact name, then prefix, then token/substring, then alphabetical. Installed
  packages remain searchable.
- Detail pane follows the existing two-pane layout: metadata, description,
  dependencies, optional dependencies, conflicts/replaces, and reverse
  dependencies when installed.
- Primary action is `Install`, `Remove`, or a disabled `Installed`.
- Confirmation sheet before polkit: adds, removes, download size, disk delta,
  pending updates folded into the transaction, replacement warnings, and the
  reboot note. Then the existing progress and log view.
- Empty state: “Search the official repositories.” with one line stating that
  AUR and Flatpak apps are not available here.
- No results: “Not in the official repositories.” plus the query as typed.

### 13.6 Data Freshness

The helper opens a fresh libalpm handle per request, so a system update or a
repository change is visible to the next search. A missing `<repo>.db` produces
a warning on that repository rather than an empty result set.

## 14. Repository And Pacman Settings

### 14.1 Effective Configuration And Provenance

`RepositoryStatus` reads `/etc/pacman.conf` and each `Include` file in order and
reports, per repository: name, enabled state, defining file, signature level,
mirror source, package count, and the sync database modification time. The
effective values come from libalpm/pacman; the file and line come from parsing,
because Skemer must never edit a file it cannot safely own.

Distro facts that matter:

- Veldmuis already enables multilib in `/etc/pacman.conf.d/veldmuis.conf`,
  which is owned by `veldmuis-release` and included from `/etc/pacman.conf`.
  `multilib/steam` is therefore available out of the box on a current Veldmuis
  image; the toggle exists for generic Arch and for older or modified systems
  where the block is commented out.
- That file is package-owned. Skemer must not edit it, because a package update
  would silently overwrite the change. When multilib comes from it, the UI says
  “Enabled, managed by Veldmuis” and offers no disable switch.
- `core`, `extra`, and the Veldmuis repositories are never disabled from Skemer.

The Settings window gains a `System` section with the repository rows, the
multilib action, `ParallelDownloads`, and mirror refresh. The existing interval,
notification, autostart, and tray options stay in the app section.

### 14.2 Enable Multilib

- Already enabled anywhere: show enabled, no write.
- Disabled with a commented `#[multilib]` block in `/etc/pacman.conf`: uncomment
  that block in place. This is the canonical Arch location and survives pacman
  upgrades through the normal config-file handling.
- Disabled with no block: append the canonical `[multilib]` plus
  `Include = /etc/pacman.d/mirrorlist` block to `/etc/pacman.conf`, in the
  repository section, before the Veldmuis include.
- Never edit `veldmuis.conf` or any other include. Never add a second section
  when a commented one exists.
- Write atomically with the original mode preserved, a timestamped backup under
  `/var/lib/skemer/backups/`, and a rollback if validation fails.
- Validate the new file with `pacman-conf --config <candidate>`, then sync the
  databases. A sync failure restores the backup and reports the pacman error.
- Enabling multilib downloads thousands of 32-bit package entries; the UI
  states that before applying.
- Disabling a repository keeps installed packages installed but stops their
  updates. The sheet lists the installed packages that would be affected and
  requires explicit confirmation. On Veldmuis this path is not offered for
  multilib.

### 14.3 Pacman Options

Deliberately minimal in v1:

- `ParallelDownloads` (1–10) in `/etc/pacman.conf`, validated and backed up like
  the repository write. If the effective value comes from an include rather than
  `/etc/pacman.conf`, the row is read-only and names the owning file.
- Mirror refresh reuses the existing `RefreshMirrors` action and polkit action.
- Not offered: `SigLevel`, `IgnorePkg`, `IgnoreGroup`, testing repositories,
  custom repositories, `Server` entries, or any include-file editing. Holds stay
  per-transaction (§5) and are never persisted.

### 14.4 Maintenance (Later)

Package cache size and safe `pacman -Sc` cleanup, an explicit orphan list with
confirmed removal, and access to failed-transaction logs. All of these are later
than the install tab and none of them run automatically.

### 14.5 Privileges

- `SearchPackages`, `PackageInfo`, `PreviewPackages`, and `RepositoryStatus`:
  allowed for active local users, read-only, no polkit prompt.
- `ApplyPackages`: `org.veldmuislinux.skemer.manage-packages`, `auth_admin_keep`.
- `SetRepository` and the option write: separate
  `org.veldmuislinux.skemer.manage-repositories`, `auth_admin_keep`, so a policy
  can allow updates without allowing configuration changes.
- The helper writes only allowlisted paths. No request field ever names a path.
- A repository change and an install are separate transactions and separate
  authorizations.

### 14.6 Failure And Safety Rules

- No shell interpolation, no globs, no client-supplied flags.
- Requests are bounded; previews and searches are capped.
- Every config write is backup, atomic replace, validate, then rollback on error.
- A pending `.pacnew` for `pacman.conf` is surfaced as “configuration changes
  pending” and blocks Skemer writes rather than clobbering the file.
- A repository write that would leave no enabled repository, remove `core`, or
  duplicate an existing section is rejected before the filesystem is touched.

### 14.7 Testing

- Search ranking, name validation, provider rejection, group expansion, and the
  100-result cap.
- Preview closure for install and remove, including dependencies that become
  unneeded.
- Fake `pacman` assertions: installs always pass `-Syu --needed` and never `-S`,
  removals pass `-Rns` with only the previewed names, and cancellation reaches
  the process group.
- Repository parser provenance: enabled, commented, missing, and
  package-owned-include cases.
- Multilib enable: commented block, missing block, already enabled, duplicate
  avoidance, failed sync rollback, and `.pacnew` blocking.
- Playwright: enable multilib, search `steam`, preview, install, progress, and
  the read-only Veldmuis multilib state.
- One VM gate on generic Arch with multilib off (enable, then install Steam) and
  on a Veldmuis image (multilib already enabled, no toggle), plus a removal
  closure, a conflict, a group, and a cancel.

### 14.8 Non-Goals

AUR, Flatpak app installation, testing repositories, custom repositories,
mirror editing, downgrades, partial upgrades, automatic orphan removal, package
ratings, screenshots, and category browsing.

