import type {
  PackageAction,
  PackageDetails,
  PackageHit,
  PackageOutcome,
  PackagePreview,
  RepositoryInfo,
  RepositoryStatus,
  Settings,
  UpdateItem,
} from "./types";

export const DEFAULT_SETTINGS: Settings = {
  check_interval_hours: 6,
  notifications: true,
  autostart: true,
  close_to_tray: true,
  include_flatpak_system: true,
  include_flatpak_user: true,
};

export const isTauri =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export async function loadSettings(): Promise<Settings> {
  if (!isTauri) return DEFAULT_SETTINGS;
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<Settings>("load_settings");
}

export async function saveSettings(settings: Settings): Promise<void> {
  if (!isTauri) return;
  const { invoke } = await import("@tauri-apps/api/core");
  await invoke("save_settings", { settings });
}

export async function checkUpdates(): Promise<import("./types").UpdatePlan> {
  if (!isTauri) return demoPlan();
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<import("./types").UpdatePlan>("check_updates");
}

export async function applyUpdates(
  sources: string[],
  holds: string[],
): Promise<import("./types").ApplyOutcome> {
  if (!isTauri) {
    throw new Error("Apply is not implemented in the browser preview.");
  }
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<import("./types").ApplyOutcome>("apply_updates", {
    sources,
    holds,
  });
}

export async function cancelUpdates(): Promise<void> {
  if (!isTauri) return;
  const { invoke } = await import("@tauri-apps/api/core");
  await invoke("cancel_updates");
}

export async function searchPackages(
  query: string,
  installedOnly = false,
): Promise<PackageHit[]> {
  if (!isTauri) return demoSearch(query, installedOnly);
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<PackageHit[]>("search_packages", { query, installedOnly });
}

export async function packageInfo(name: string): Promise<PackageDetails> {
  if (!isTauri) return demoDetails(name);
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<PackageDetails>("package_info", { name });
}

export async function previewPackages(
  action: PackageAction,
  names: string[],
): Promise<PackagePreview> {
  if (!isTauri) return demoPreview(action, names);
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<PackagePreview>("preview_packages", { action, names });
}

export async function applyPackages(
  action: PackageAction,
  names: string[],
): Promise<PackageOutcome> {
  if (!isTauri) {
    throw new Error("Package changes are not implemented in the browser preview.");
  }
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<PackageOutcome>("apply_packages", { action, names });
}

export async function loadRepositoryStatus(): Promise<RepositoryStatus> {
  if (!isTauri) return demoRepositories();
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<RepositoryStatus>("repository_status");
}

export async function setRepository(
  name: string,
  enabled: boolean,
): Promise<RepositoryStatus> {
  if (!isTauri) {
    const status = demoRepositories();
    const repo = status.repositories.find((entry) => entry.name === name);
    if (repo) repo.enabled = enabled;
    return status;
  }
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<RepositoryStatus>("set_repository", { name, enabled });
}

export async function setPacmanOption(
  name: string,
  value: string,
): Promise<RepositoryStatus> {
  if (!isTauri) {
    const status = demoRepositories();
    status.parallel_downloads = Number(value);
    return status;
  }
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<RepositoryStatus>("set_pacman_option", { name, value });
}

function demoPlan(): import("./types").UpdatePlan {
  const items = [
    ["linux", "core", "6.17.7.arch1-1", "6.17.8.arch1-1", 142, true, "Linux kernel and modules"],
    ["systemd", "core", "257.8-1", "257.9-1", 18, true, "System and service manager"],
    ["mesa", "extra", "25.2.4-1", "25.2.5-1", 38, true, "Open-source graphics stack"],
    ["firefox", "extra", "144.0.2-1", "145.0-1", 76, false, "Standalone web browser"],
    ["git", "extra", "2.56.0-1", "2.56.1-1", 8, false, "Distributed version control"],
    ["python", "extra", "3.13.9-1", "3.13.10-1", 12, false, "Python interpreter"],
    ["libxml2", "core", "2.15.1-1", "2.15.2-1", 1.2, false, "XML parsing library"],
    ["veldmuis-branding", "veldmuis-core", "20260306-23", "20260306-24", 2.4, false, "Veldmuis branding assets"],
  ].map(([name, repo, current, next, size, reboot, summary]) => ({
    id: `system:${name}`,
    source: "system" as const,
    name: String(name),
    repo: String(repo),
    current_version: String(current),
    new_version: String(next),
    download_bytes: Number(size) * 1024 * 1024,
    installed_bytes: 0,
    summary: String(summary),
    reboot_suggested: Boolean(reboot),
    warning: null,
  }));

  const flatpak = [
    ["org.gimp.GIMP", "3.0.2", "3.0.4", 64, "system"],
    ["org.freedesktop.Platform", "24.08", "24.08.2", 73, "system"],
    ["com.spotify.Client", "1.2.63", "1.2.64", 112, "user"],
    ["org.keepassxc.KeePassXC", "2.7.9", "2.7.10", 28, "user"],
  ].map(([name, current, next, size, scope]) => ({
    id: `flatpak:${scope}:${name}`,
    source: (scope === "system" ? "flatpak-system" : "flatpak-user") as UpdateItem["source"],
    name: String(name),
    repo: "flathub",
    current_version: String(current),
    new_version: String(next),
    download_bytes: Number(size) * 1024 * 1024,
    installed_bytes: 0,
    summary: "Flatpak application or runtime",
    reboot_suggested: false,
    warning: null,
  }));

  const all: UpdateItem[] = [...items, ...flatpak];
  return {
    items: all,
    total_download_bytes: all.reduce((sum, item) => sum + item.download_bytes, 0),
    reboot_suggested: all.some((item) => item.reboot_suggested),
    checked_at: "browser preview",
    warnings: [],
    held_packages: [],
  };
}

const DEMO_PACKAGES: PackageHit[] = [
  {
    name: "steam",
    repo: "multilib",
    version: "1.0.0.87-3",
    installed_version: null,
    download_bytes: 4.7 * 1024 * 1024,
    installed_bytes: 12.1 * 1024 * 1024,
    summary: "Valve's digital software delivery system",
    groups: [],
  },
  {
    name: "openai-codex",
    repo: "extra",
    version: "0.169.0-1",
    installed_version: null,
    download_bytes: 18.2 * 1024 * 1024,
    installed_bytes: 61.4 * 1024 * 1024,
    summary: "OpenAI's lightweight coding agent that runs in your terminal",
    groups: [],
  },
  {
    name: "firefox",
    repo: "extra",
    version: "145.0-1",
    installed_version: "144.0.2-1",
    download_bytes: 76 * 1024 * 1024,
    installed_bytes: 240 * 1024 * 1024,
    summary: "Standalone web browser",
    groups: [],
  },
  {
    name: "gimp",
    repo: "extra",
    version: "3.0.4-1",
    installed_version: null,
    download_bytes: 24 * 1024 * 1024,
    installed_bytes: 98 * 1024 * 1024,
    summary: "GNU Image Manipulation Program",
    groups: [],
  },
  {
    name: "vlc",
    repo: "extra",
    version: "3.0.21-12",
    installed_version: null,
    download_bytes: 14 * 1024 * 1024,
    installed_bytes: 52 * 1024 * 1024,
    summary: "Multi-platform MPEG, VCD/DVD, and DivX player",
    groups: [],
  },
  {
    name: "lib32-glibc",
    repo: "multilib",
    version: "2.42-4",
    installed_version: null,
    download_bytes: 3.1 * 1024 * 1024,
    installed_bytes: 14 * 1024 * 1024,
    summary: "GNU C Library (32-bit)",
    groups: [],
  },
];

function demoSearch(query: string, installedOnly: boolean): PackageHit[] {
  const needle = query.trim().toLowerCase();
  return DEMO_PACKAGES.filter((item) => {
    if (installedOnly && item.installed_version === null) return false;
    if (!needle) return true;
    return (
      item.name.toLowerCase().includes(needle) ||
      item.summary.toLowerCase().includes(needle)
    );
  }).map((item) => ({ ...item }));
}

function demoDetails(name: string): PackageDetails {
  const hit = DEMO_PACKAGES.find((item) => item.name === name) ?? DEMO_PACKAGES[0];
  return {
    name: hit.name,
    repo: hit.repo,
    version: hit.version,
    installed_version: hit.installed_version,
    description: hit.summary,
    url: "https://example.invalid",
    packager: "Veldmuis Build System",
    build_date: 0,
    download_bytes: hit.download_bytes,
    installed_bytes: hit.installed_bytes,
    licenses: ["GPL"],
    groups: hit.groups,
    depends: ["glibc", "zlib"],
    optdepends: ["ffmpeg: media playback"],
    conflicts: [],
    replaces: [],
    provides: [],
    required_by: [],
  };
}

function demoPreview(action: PackageAction, names: string[]): PackagePreview {
  const entries = names.map((name) => demoDetails(name));
  const adds: PackagePreview["adds"] =
    action === "install"
      ? entries.map((entry) => ({
          name: entry.name,
          repo: entry.repo,
          version: entry.version,
          current_version: entry.installed_version,
          download_bytes: entry.download_bytes,
          installed_bytes: entry.installed_bytes,
        }))
      : [];
  const removes: PackagePreview["removes"] =
    action === "remove"
      ? entries.map((entry) => ({
          name: entry.name,
          repo: entry.repo,
          version: entry.version,
          current_version: entry.installed_version,
          download_bytes: 0,
          installed_bytes: entry.installed_bytes,
        }))
      : [];
  return {
    action,
    adds,
    removes,
    total_download_bytes: adds.reduce((sum, item) => sum + item.download_bytes, 0),
    total_installed_bytes: (action === "install" ? adds : removes).reduce(
      (sum, item) => sum + item.installed_bytes,
      0,
    ),
    pending_system_updates: action === "install" ? 3 : 0,
    reboot_suggested: false,
    warnings:
      action === "install"
        ? ["This transaction also applies 3 pending system update(s)."]
        : [],
  };
}

function demoRepositories(): RepositoryStatus {
  const entry = (
    name: string,
    enabled: boolean,
    toggleable: boolean,
    note: string | null,
    count: number,
  ): RepositoryInfo => ({
    name,
    enabled,
    source_file: enabled
      ? "/etc/pacman.conf.d/veldmuis.conf"
      : "/etc/pacman.conf",
    note,
    toggleable,
    package_count: count,
    synced_at: "2026-11-24 09:00",
  });
  return {
    pacman_conf: "/etc/pacman.conf",
    repositories: [
      entry("core", true, false, "Required repository.", 287),
      entry("extra", true, false, "Required repository.", 14_912),
      entry(
        "multilib",
        true,
        false,
        "Enabled by /etc/pacman.conf.d/veldmuis.conf.",
        590,
      ),
      entry("veldmuis-core", true, false, "Required repository.", 12),
      entry("veldmuis-extra", true, false, "Required repository.", 8),
    ],
    warnings: [],
    parallel_downloads: 5,
    parallel_downloads_editable: true,
    parallel_downloads_source: "/etc/pacman.conf",
  };
}
