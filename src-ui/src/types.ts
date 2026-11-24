export type UpdateSource = "system" | "flatpak-system" | "flatpak-user";

export interface UpdateItem {
  id: string;
  source: UpdateSource;
  name: string;
  repo: string;
  current_version: string;
  new_version: string;
  download_bytes: number;
  installed_bytes: number;
  summary: string;
  reboot_suggested: boolean;
  warning: string | null;
}

export interface UpdatePlan {
  items: UpdateItem[];
  total_download_bytes: number;
  reboot_suggested: boolean;
  checked_at: string;
  warnings: string[];
  held_packages: string[];
}

export interface Settings {
  check_interval_hours: number;
  notifications: boolean;
  autostart: boolean;
  close_to_tray: boolean;
  include_flatpak_system: boolean;
  include_flatpak_user: boolean;
}

export type PackageAction = "install" | "remove";

export interface PackageHit {
  name: string;
  repo: string;
  version: string;
  installed_version: string | null;
  download_bytes: number;
  installed_bytes: number;
  summary: string;
  groups: string[];
}

export interface PackageDetails {
  name: string;
  repo: string | null;
  version: string;
  installed_version: string | null;
  description: string;
  url: string;
  packager: string;
  build_date: number;
  download_bytes: number;
  installed_bytes: number;
  licenses: string[];
  groups: string[];
  depends: string[];
  optdepends: string[];
  conflicts: string[];
  replaces: string[];
  provides: string[];
  required_by: string[];
}

export interface PackagePreviewItem {
  name: string;
  repo: string | null;
  version: string;
  current_version: string | null;
  download_bytes: number;
  installed_bytes: number;
}

export interface PackagePreview {
  action: PackageAction;
  adds: PackagePreviewItem[];
  removes: PackagePreviewItem[];
  total_download_bytes: number;
  total_installed_bytes: number;
  pending_system_updates: number;
  reboot_suggested: boolean;
  warnings: string[];
}

export interface PackageOutcome {
  success: boolean;
  cancelled: boolean;
  action: PackageAction;
  names: string[];
  reboot_suggested: boolean;
  log_tail: string;
  error: string | null;
}

export interface RepositoryInfo {
  name: string;
  enabled: boolean;
  source_file: string;
  note: string | null;
  toggleable: boolean;
  package_count: number;
  synced_at: string | null;
}

export interface RepositoryStatus {
  pacman_conf: string;
  repositories: RepositoryInfo[];
  warnings: string[];
  parallel_downloads: number;
  parallel_downloads_editable: boolean;
  parallel_downloads_source: string | null;
}

export interface ApplyOutcome {
  success: boolean;
  cancelled: boolean;
  held_packages: string[];
  reboot_suggested: boolean;
  log_tail: string;
  error: string | null;
  candidates: string[];
}

export const SOURCE_LABELS: Record<UpdateSource, string> = {
  system: "System",
  "flatpak-system": "Flatpak system",
  "flatpak-user": "Flatpak user",
};

export function sourceLabel(source: UpdateSource): string {
  return SOURCE_LABELS[source] ?? source;
}

export function packageActionLabel(action: PackageAction): string {
  return action === "install" ? "Install" : "Remove";
}

export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  if (bytes >= 1024 ** 3) return `${(bytes / 1024 ** 3).toFixed(2)} GiB`;
  if (bytes >= 1024 ** 2) return `${(bytes / 1024 ** 2).toFixed(1)} MiB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(0)} KiB`;
  return `${bytes} B`;
}
