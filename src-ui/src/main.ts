import "./style.css";
import {
  applyPackages,
  applyUpdates,
  cancelUpdates,
  checkUpdates,
  DEFAULT_SETTINGS,
  isTauri,
  loadRepositoryStatus,
  loadSettings,
  packageInfo,
  previewPackages,
  saveSettings,
  searchPackages,
  setPacmanOption,
  setRepository,
} from "./api";
import {
  formatBytes,
  packageActionLabel,
  sourceLabel,
  type ApplyOutcome,
  type PackageAction,
  type PackageDetails,
  type PackageHit,
  type PackagePreview,
  type RepositoryStatus,
  type Settings,
  type UpdateItem,
  type UpdatePlan,
  type UpdateSource,
} from "./types";

type ActiveTab = "system" | "flatpak";
type View = "updates" | "browse";

interface State {
  plan: UpdatePlan | null;
  active: ActiveTab;
  view: View;
  selected: Set<UpdateSource>;
  detail: string | null;
  settings: Settings;
  loading: boolean;
  error: string | null;
  notice: string | null;
  settingsOpen: boolean;
  applying: boolean;
  cancelRequested: boolean;
  progress: number;
  progressMessage: string;
  operationTitle: string;
  log: string[];
  searchQuery: string;
  searchResults: PackageHit[];
  searchLoading: boolean;
  searchFocused: boolean;
  selectedPackage: string | null;
  packageDetails: PackageDetails | null;
  packagePreview: PackagePreview | null;
  packageBusy: boolean;
  repositoryStatus: RepositoryStatus | null;
  repositoryBusy: boolean;
}

const state: State = {
  plan: null,
  active: "system",
  view: "updates",
  selected: new Set(["system", "flatpak-system", "flatpak-user"]),
  detail: null,
  settings: DEFAULT_SETTINGS,
  loading: false,
  error: null,
  notice: null,
  settingsOpen: false,
  applying: false,
  cancelRequested: false,
  progress: 0,
  progressMessage: "Preparing transaction…",
  operationTitle: "Updating the system",
  log: [],
  searchQuery: "",
  searchResults: [],
  searchLoading: false,
  searchFocused: false,
  selectedPackage: null,
  packageDetails: null,
  packagePreview: null,
  packageBusy: false,
  repositoryStatus: null,
  repositoryBusy: false,
};

const appElement = document.querySelector<HTMLDivElement>("#app");
if (!appElement) throw new Error("missing #app");
const app: HTMLDivElement = appElement;

function escapeHtml(value: string): string {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

function selectedItems(): UpdateItem[] {
  if (!state.plan) return [];
  return state.plan.items.filter((item) => state.selected.has(item.source));
}

function itemsFor(source: UpdateSource): UpdateItem[] {
  if (!state.plan) return [];
  return state.plan.items.filter((item) => item.source === source);
}

function itemsForTab(tab: ActiveTab): UpdateItem[] {
  if (!state.plan) return [];
  if (tab === "system") {
    return state.plan.items.filter((item) => item.source === "system");
  }
  return state.plan.items.filter(
    (item) => item.source === "flatpak-system" || item.source === "flatpak-user",
  );
}

function flatpakItems(source: "flatpak-system" | "flatpak-user"): UpdateItem[] {
  return itemsFor(source);
}

function tabCount(tab: ActiveTab): number {
  return itemsForTab(tab).length;
}

function sourceCount(source: UpdateSource): number {
  return itemsFor(source).length;
}

function currentDetail(): UpdateItem | undefined {
  const items = itemsForTab(state.active);
  return items.find((item) => item.id === state.detail) ?? items[0];
}

function applySettingsToSelection(settings: Settings): void {
  if (settings.include_flatpak_system) state.selected.add("flatpak-system");
  else state.selected.delete("flatpak-system");
  if (settings.include_flatpak_user) state.selected.add("flatpak-user");
  else state.selected.delete("flatpak-user");
}

function rowHtml(item: UpdateItem, detail: UpdateItem | undefined): string {
  return `
    <button class="row ${detail?.id === item.id ? "selected" : ""}" data-detail="${escapeHtml(item.id)}">
      <span class="pkg">
        <span class="badge">${escapeHtml(item.repo)}</span>
        <span class="pkg-name">${escapeHtml(item.name)}</span>
        <span class="pkg-summary">${escapeHtml(item.summary)}</span>
      </span>
      <span class="ver"><span>${escapeHtml(item.current_version)}</span><span class="arrow">→</span><span class="new">${escapeHtml(item.new_version)}</span></span>
      <span class="size">${formatBytes(item.download_bytes)}</span>
    </button>`;
}

function flatpakGroup(
  source: "flatpak-system" | "flatpak-user",
  label: string,
  detail: UpdateItem | undefined,
): string {
  const items = flatpakItems(source);
  if (items.length === 0) return "";
  return `
    <div class="group-head">${label} <span class="count-pill">${items.length}</span></div>
    ${items.map((item) => rowHtml(item, detail)).join("")}`;
}

let renderQueued = false;
let autoRefreshTimer: number | null = null;
let searchTimer: number | null = null;

function scheduleAutoRefresh(): void {
  if (autoRefreshTimer !== null) {
    window.clearInterval(autoRefreshTimer);
    autoRefreshTimer = null;
  }
  if (!isTauri) return;
  const hours = state.settings.check_interval_hours;
  if (hours <= 0) return;
  autoRefreshTimer = window.setInterval(
    () => {
      if (!state.applying && !state.loading) void refresh();
    },
    hours * 60 * 60 * 1000,
  );
}

function scheduleRender(): void {
  if (renderQueued) return;
  renderQueued = true;
  requestAnimationFrame(() => {
    renderQueued = false;
    render();
  });
}

function logLineClass(line: string): string {
  const lower = line.toLowerCase();
  if (lower.includes("error:") || lower.includes("failed")) return "err";
  if (lower.includes("warning:")) return "warn";
  if (line.startsWith("::")) return "ok";
  return "";
}

function progressView(): string {
  const lines = state.log.slice(-200);
  const packageAction = state.operationTitle.startsWith("Install")
    ? "install"
    : state.operationTitle.startsWith("Remove")
      ? "remove"
      : "update";
  return `
    <section class="panel progress-panel">
      <div class="progress-head">
        <div>
          <h2>${escapeHtml(state.operationTitle)}</h2>
          <p>Do not power off the system during the ${packageAction}.</p>
        </div>
        <span class="chip progress-chip">${state.progress}%</span>
      </div>
      <div class="bar"><i style="width:${state.progress}%"></i></div>
      <div class="progress-meta">
        <span>${escapeHtml(state.progressMessage || "Preparing transaction…")}</span>
        <span>${state.progress}%</span>
      </div>
      <div class="log" id="update-log">${lines
        .map((line) => `<span class="${logLineClass(line)}">${escapeHtml(line)}</span>`)
        .join("\n")}</div>
      <div class="progress-actions">
        <button class="btn ghost" id="cancel" ${state.cancelRequested ? "disabled" : ""}>${state.cancelRequested ? "Cancelling…" : "Cancel"}</button>
      </div>
    </section>`;
}

function updatesView(
  selected: UpdateItem[],
  total: number,
  reboot: boolean,
  detail: UpdateItem | undefined,
): string {
  return `
      <section class="summary ${state.applying ? "hidden" : ""}">
        <div class="count">${selected.length}</div>
        <div class="summary-text">
          <strong>${selected.length === 0 ? "No selected updates" : "updates selected"}</strong>
          <span>${formatBytes(total)} to download</span>
        </div>
        <div class="summary-actions">
          ${reboot ? `<span class="chip reboot">Reboot suggested</span>` : ""}
          <button class="btn primary" id="update" ${state.loading || selected.length === 0 ? "disabled" : ""}>Update all</button>
        </div>
      </section>

      <nav class="tabs ${state.applying ? "hidden" : ""}">
        <button class="tab ${state.active === "system" ? "active" : ""}" data-tab="system">
          System <span class="count-pill">${tabCount("system")}</span>
        </button>
        <button class="tab ${state.active === "flatpak" ? "active" : ""}" data-tab="flatpak">
          Flatpak <span class="count-pill">${tabCount("flatpak")}</span>
        </button>
      </nav>

      <section class="workspace ${state.applying ? "hidden" : ""}">
        <div class="panel list-panel">
          ${
            state.active === "system"
              ? `
                <div class="list-head">
                  <label class="source-toggle">
                    <input type="checkbox" data-toggle-source="system" ${state.selected.has("system") ? "checked" : ""}>
                    Include System
                  </label>
                  <span>Version</span>
                  <span class="right">Size</span>
                </div>
                <div class="list">
                  ${
                    itemsForTab("system").length === 0
                      ? `<div class="empty"><strong>Nothing to update</strong>The system is up to date.</div>`
                      : itemsForTab("system")
                          .map((item) => rowHtml(item, detail))
                          .join("")
                  }
                </div>`
              : `
                <div class="list-head flatpak-head">
                  <div class="source-toggles">
                    <label class="source-toggle">
                      <input type="checkbox" data-toggle-source="flatpak-system" ${state.selected.has("flatpak-system") ? "checked" : ""}> System
                    </label>
                    <label class="source-toggle">
                      <input type="checkbox" data-toggle-source="flatpak-user" ${state.selected.has("flatpak-user") ? "checked" : ""}> User
                    </label>
                  </div>
                  <span>Version</span>
                  <span class="right">Size</span>
                </div>
                <div class="list">
                  ${
                    itemsForTab("flatpak").length === 0
                      ? `<div class="empty"><strong>Nothing to update</strong>Flatpak is up to date.</div>`
                      : flatpakGroup("flatpak-system", "System", detail) +
                        flatpakGroup("flatpak-user", "User", detail)
                  }
                </div>`
          }
        </div>

        <aside class="panel details">
          ${
            detail
              ? `
                <div class="eyebrow">Selected package</div>
                <h2>${escapeHtml(detail.name)}</h2>
                <div class="repo"><span class="badge">${escapeHtml(detail.repo)}</span>${escapeHtml(detail.summary)}</div>
                <p class="desc">This update moves <b>${escapeHtml(detail.name)}</b> from <code>${escapeHtml(detail.current_version)}</code> to <code>${escapeHtml(detail.new_version)}</code>.</p>
                <dl class="kv">
                  <dt>Source</dt><dd>${escapeHtml(sourceLabel(detail.source))}</dd>
                  <dt>Download</dt><dd>${formatBytes(detail.download_bytes)}</dd>
                  <dt>Reboot</dt><dd>${detail.reboot_suggested ? "Suggested" : "No"}</dd>
                </dl>
                ${detail.reboot_suggested ? `<div class="warn">Reboot suggested after updating. The system keeps running until you reboot.</div>` : ""}
              `
              : `<div class="empty"><strong>No package selected</strong>Select a package to see details.</div>`
          }
        </aside>
      </section>`;
}

function packageRowHtml(item: PackageHit): string {
  const installed = item.installed_version !== null;
  return `
    <button class="row ${state.selectedPackage === item.name ? "selected" : ""}" data-package="${escapeHtml(item.name)}">
      <span class="pkg">
        <span class="pkg-title">
          <span class="badge ${item.repo === "multilib" ? "multilib" : ""}">${escapeHtml(item.repo)}</span>
          <span class="pkg-name">${escapeHtml(item.name)}</span>
          ${installed ? `<span class="chip installed">Installed</span>` : ""}
        </span>
        <span class="pkg-summary">${escapeHtml(item.summary)}</span>
      </span>
      <span class="ver">${escapeHtml(item.version)}</span>
      <span class="size">${formatBytes(item.download_bytes)}</span>
    </button>`;
}

function dependencyBlock(title: string, values: string[]): string {
  if (values.length === 0) return "";
  const shown = values.slice(0, 8);
  const extra = values.length - shown.length;
  return `
    <div class="dep-block">
      <div class="eyebrow">${escapeHtml(title)}</div>
      <div class="dep-list">
        ${shown.map((value) => `<span>${escapeHtml(value)}</span>`).join("")}
        ${extra > 0 ? `<span class="muted">+${extra} more</span>` : ""}
      </div>
    </div>`;
}

function browseDetailsHtml(): string {
  const details = state.packageDetails;
  if (!details) {
    return `<div class="empty"><strong>No package selected</strong>Search the official repositories to see details.</div>`;
  }
  const installed = details.installed_version !== null;
  const action: PackageAction = installed ? "remove" : "install";
  return `
    <div class="eyebrow">${installed ? "Installed package" : "Available package"}</div>
    <h2>${escapeHtml(details.name)}</h2>
    <div class="repo"><span class="badge ${details.repo === "multilib" ? "multilib" : ""}">${escapeHtml(details.repo ?? "local")}</span>${escapeHtml(details.description)}</div>
    <dl class="kv">
      <dt>Version</dt><dd>${escapeHtml(details.version)}</dd>
      <dt>Installed</dt><dd>${details.installed_version ? escapeHtml(details.installed_version) : "No"}</dd>
      <dt>Download</dt><dd>${formatBytes(details.download_bytes)}</dd>
      <dt>Installed size</dt><dd>${formatBytes(details.installed_bytes)}</dd>
      ${details.groups.length > 0 ? `<dt>Group</dt><dd>${escapeHtml(details.groups.join(", "))}</dd>` : ""}
    </dl>
    ${dependencyBlock("Depends", details.depends)}
    ${dependencyBlock("Optional", details.optdepends)}
    ${dependencyBlock("Conflicts", details.conflicts)}
    ${dependencyBlock("Replaces", details.replaces)}
    ${dependencyBlock("Required by", details.required_by)}
    ${installed && details.required_by.length > 0 ? `<div class="warn">Removing this package will also remove or break the packages listed above. The confirmation lists the full removal set.</div>` : ""}
    <button class="btn primary wide" id="package-action" data-action="${action}" ${state.packageBusy ? "disabled" : ""}>
      ${state.packageBusy ? "Preparing…" : installed ? "Remove" : "Install"}
    </button>`;
}

function browseView(): string {
  const results = state.searchResults;
  const empty = state.searchLoading
    ? `<div class="empty"><strong>Searching…</strong></div>`
    : state.searchQuery.trim()
      ? `<div class="empty"><strong>Not in the official repositories</strong>AUR and Flatpak apps are not available here.</div>`
      : `<div class="empty"><strong>Search the official repositories</strong>AUR and Flatpak apps are not available here.</div>`;

  return `
      <section class="workspace">
        <div class="panel list-panel">
          <div class="search-bar">
            <input id="package-search" type="search" placeholder="Search the official repositories" value="${escapeHtml(state.searchQuery)}" autocomplete="off" spellcheck="false">
            <span class="muted">${state.searchLoading ? "Searching…" : `${results.length} shown`}</span>
          </div>
          <div class="list-head browse-head">
            <span>Package</span>
            <span>Version</span>
            <span class="right">Size</span>
          </div>
          <div class="list">
            ${results.length === 0 ? empty : results.map(packageRowHtml).join("")}
          </div>
        </div>

        <aside class="panel details">
          ${browseDetailsHtml()}
        </aside>
      </section>`;
}

function repositoryRowHtml(repo: RepositoryStatus["repositories"][number]): string {
  const action = repo.enabled ? "Disable" : "Enable";
  return `
    <div class="repo-row">
      <div class="repo-row-main">
        <span class="badge ${repo.name === "multilib" ? "multilib" : ""}">${escapeHtml(repo.name)}</span>
        <span class="repo-state ${repo.enabled ? "on" : "off"}">${repo.enabled ? "Enabled" : "Disabled"}</span>
        <span class="muted">${repo.package_count} packages${repo.synced_at ? ` · synced ${escapeHtml(repo.synced_at)}` : ""}</span>
      </div>
      <div class="repo-row-side">
        ${repo.note ? `<span class="muted">${escapeHtml(repo.note)}</span>` : ""}
        ${
          repo.toggleable
            ? `<button class="btn small" data-repo="${escapeHtml(repo.name)}" data-enable="${repo.enabled ? "false" : "true"}" ${state.repositoryBusy ? "disabled" : ""}>${action}</button>`
            : ""
        }
      </div>
    </div>`;
}

function settingsView(): string {
  const repositories = state.repositoryStatus;
  return `
    <section class="settings panel">
      <h2>Settings</h2>
      <label>Check for updates
        <select id="setting-interval">
          ${[0, 1, 3, 6, 12, 24]
            .map(
              (hours) =>
                `<option value="${hours}" ${state.settings.check_interval_hours === hours ? "selected" : ""}>${hours === 0 ? "Never" : `Every ${hours} hour${hours === 1 ? "" : "s"}`}</option>`,
            )
            .join("")}
        </select>
      </label>
      <label><input type="checkbox" id="setting-notifications" ${state.settings.notifications ? "checked" : ""}> Desktop notifications</label>
      <label><input type="checkbox" id="setting-autostart" ${state.settings.autostart ? "checked" : ""}> Start at login</label>
      <label><input type="checkbox" id="setting-close-to-tray" ${state.settings.close_to_tray ? "checked" : ""}> Close to tray</label>
      <label><input type="checkbox" id="setting-flatpak-system" ${state.settings.include_flatpak_system ? "checked" : ""}> Include Flatpak system</label>
      <label><input type="checkbox" id="setting-flatpak-user" ${state.settings.include_flatpak_user ? "checked" : ""}> Include Flatpak user</label>

      <h3 class="settings-heading">Repositories</h3>
      ${
        repositories
          ? `<div class="repo-list">${repositories.repositories.map(repositoryRowHtml).join("")}</div>
             ${repositories.parallel_downloads_editable
               ? `<label class="inline-setting">Parallel downloads
                    <input id="setting-parallel" type="number" min="1" max="10" value="${repositories.parallel_downloads}">
                    <button class="btn small" id="setting-parallel-apply" ${state.repositoryBusy ? "disabled" : ""}>Apply</button>
                  </label>`
               : `<p class="muted">ParallelDownloads is managed by ${escapeHtml(repositories.parallel_downloads_source ?? "another file")}.</p>`}
             ${repositories.warnings.map((warning) => `<div class="warn">${escapeHtml(warning)}</div>`).join("")}`
          : `<p class="muted">Loading repository state…</p>`
      }

      <div class="settings-actions">
        <button class="btn ghost" id="settings-cancel">Cancel</button>
        <button class="btn primary" id="settings-save">Save</button>
      </div>
    </section>`;
}

function previewView(): string {
  const preview = state.packagePreview;
  if (!preview) return "";
  const items = preview.action === "install" ? preview.adds : preview.removes;
  const empty = items.length === 0;
  return `
    <div class="overlay">
      <section class="panel dialog">
        <div class="eyebrow">${packageActionLabel(preview.action)}</div>
        <h2>${packageActionLabel(preview.action)} ${escapeHtml(state.selectedPackage ?? items[0]?.name ?? "package")}</h2>
        <p class="desc">${
          empty
            ? "Nothing would change. The package is already at the repository version."
            : preview.action === "install"
              ? "The transaction below is resolved with libalpm and applied as one full system sync."
              : "The transaction below is the complete removal set, including dependencies that become unneeded."
        }</p>
        ${
          empty
            ? ""
            : `<div class="transaction">
                ${items
                  .map(
                    (item) => `
                      <div class="transaction-row">
                        <span>${escapeHtml(item.name)}</span>
                        <span class="muted">${item.current_version ? `${escapeHtml(item.current_version)} → ` : ""}${escapeHtml(item.version)}</span>
                        <span class="muted">${preview.action === "install" ? formatBytes(item.download_bytes) : formatBytes(item.installed_bytes)}</span>
                      </div>`,
                  )
                  .join("")}
              </div>`
        }
        <dl class="kv">
          ${preview.action === "install" ? `<dt>Download</dt><dd>${formatBytes(preview.total_download_bytes)}</dd>` : `<dt>Freed</dt><dd>${formatBytes(preview.total_installed_bytes)}</dd>`}
          ${preview.pending_system_updates > 0 ? `<dt>System updates</dt><dd>${preview.pending_system_updates} folded in</dd>` : ""}
          <dt>Reboot</dt><dd>${preview.reboot_suggested ? "Suggested" : "No"}</dd>
        </dl>
        ${preview.warnings.map((warning) => `<div class="warn">${escapeHtml(warning)}</div>`).join("")}
        <div class="settings-actions">
          <button class="btn ghost" id="preview-cancel">Cancel</button>
          <button class="btn primary" id="preview-confirm" ${empty || state.packageBusy ? "disabled" : ""}>${packageActionLabel(preview.action)}</button>
        </div>
      </section>
    </div>`;
}

function render(): void {
  const selected = selectedItems();
  const total = selected.reduce((sum, item) => sum + item.download_bytes, 0);
  const reboot = selected.some((item) => item.reboot_suggested);
  const detail = currentDetail();
  const browse = state.view === "browse";

  app.innerHTML = `
    <div class="app">
      <header class="header">
        <div>
          <h1>Skemer</h1>
          <p>${browse ? "Official repositories" : state.plan ? `Last checked <b>${escapeHtml(state.plan.checked_at)}</b>` : "Veldmuis updater"}</p>
        </div>
        <div class="header-actions">
          <div class="mode-switch">
            <button class="mode ${!browse ? "active" : ""}" data-view="updates" ${state.applying ? "disabled" : ""}>Updates</button>
            <button class="mode ${browse ? "active" : ""}" data-view="browse" ${state.applying ? "disabled" : ""}>Browse</button>
          </div>
          <button class="btn ghost" id="settings" title="Settings" ${state.applying ? "disabled" : ""}>Settings</button>
          ${browse ? "" : `<button class="btn" id="check" ${state.loading || state.applying ? "disabled" : ""}>${state.loading ? "Checking…" : "Check for updates"}</button>`}
        </div>
      </header>

      ${state.error ? `<div class="banner error">${escapeHtml(state.error)}</div>` : ""}
      ${state.notice ? `<div class="banner success">${escapeHtml(state.notice)}</div>` : ""}

      ${state.applying ? "" : browse ? browseView() : updatesView(selected, total, reboot, detail)}

      ${state.applying ? progressView() : ""}

      <footer class="footer">
        <span class="muted">${isTauri ? "Skemer is running natively." : "Browser preview with sample data."}</span>
        <span class="spacer"></span>
        <span class="muted">${browse ? `${state.searchResults.length} packages` : state.plan ? `${state.plan.items.length} total updates` : "No plan yet"}</span>
      </footer>

      ${state.packagePreview && !state.applying ? previewView() : ""}
      ${state.settingsOpen && !state.applying ? settingsView() : ""}
    </div>`;

  wire();
  restoreSearchFocus();

  if (state.applying) {
    const logElement = document.querySelector<HTMLDivElement>("#update-log");
    if (logElement) logElement.scrollTop = logElement.scrollHeight;
  }
}

function restoreSearchFocus(): void {
  if (state.view !== "browse" || !state.searchFocused) return;
  const input = document.querySelector<HTMLInputElement>("#package-search");
  if (!input) return;
  input.focus();
  const end = input.value.length;
  input.setSelectionRange(end, end);
}

function wire(): void {
  document.querySelector("#check")?.addEventListener("click", () => {
    state.notice = null;
    void refresh();
  });
  document.querySelector("#update")?.addEventListener("click", () => void apply());
  document.querySelector("#cancel")?.addEventListener("click", () => void cancel());
  document.querySelector("#settings")?.addEventListener("click", () => void openSettings());
  document.querySelector("#settings-cancel")?.addEventListener("click", () => {
    state.settingsOpen = false;
    render();
  });
  document.querySelector("#settings-save")?.addEventListener("click", () => void saveSettingsFromForm());
  document.querySelector("#setting-parallel-apply")?.addEventListener("click", () => void applyParallelDownloads());

  document.querySelectorAll<HTMLElement>("[data-view]").forEach((element) => {
    element.addEventListener("click", () => {
      const view = element.dataset.view as View;
      if (state.view === view) return;
      state.view = view;
      state.error = null;
      state.notice = null;
      if (view === "browse") {
        state.searchFocused = true;
        if (state.searchResults.length === 0) void runSearch();
      }
      render();
    });
  });

  document.querySelectorAll<HTMLElement>("[data-tab]").forEach((element) => {
    element.addEventListener("click", () => {
      state.active = element.dataset.tab as ActiveTab;
      state.detail = null;
      render();
    });
  });

  document.querySelectorAll<HTMLInputElement>("[data-toggle-source]").forEach((element) => {
    element.addEventListener("change", () => {
      const source = element.dataset.toggleSource as UpdateSource;
      if (element.checked) state.selected.add(source);
      else state.selected.delete(source);
      render();
    });
  });

  document.querySelectorAll<HTMLElement>("[data-detail]").forEach((element) => {
    element.addEventListener("click", () => {
      state.detail = element.dataset.detail ?? null;
      render();
    });
  });

  const search = document.querySelector<HTMLInputElement>("#package-search");
  search?.addEventListener("focus", () => {
    state.searchFocused = true;
  });
  search?.addEventListener("blur", () => {
    state.searchFocused = false;
  });
  search?.addEventListener("input", () => {
    state.searchQuery = search.value;
    if (searchTimer !== null) window.clearTimeout(searchTimer);
    searchTimer = window.setTimeout(() => {
      searchTimer = null;
      void runSearch();
    }, 180);
  });

  document.querySelectorAll<HTMLElement>("[data-package]").forEach((element) => {
    element.addEventListener("click", () => {
      state.searchFocused = false;
      const name = element.dataset.package;
      if (name) void loadPackage(name);
    });
  });

  document.querySelector("#package-action")?.addEventListener("click", (event) => {
    const action = (event.currentTarget as HTMLElement).dataset.action as PackageAction;
    void openPreview(action);
  });

  document.querySelector("#preview-cancel")?.addEventListener("click", () => {
    state.packagePreview = null;
    render();
  });
  document.querySelector("#preview-confirm")?.addEventListener("click", () => void confirmPreview());

  document.querySelectorAll<HTMLElement>("[data-repo]").forEach((element) => {
    element.addEventListener("click", () => {
      const name = element.dataset.repo;
      if (!name) return;
      void toggleRepository(name, element.dataset.enable === "true");
    });
  });
}

async function refresh(): Promise<void> {
  state.loading = true;
  state.error = null;
  render();
  try {
    state.plan = await checkUpdates();
    state.detail = currentDetail()?.id ?? null;
  } catch (error) {
    state.error = String(error);
  } finally {
    state.loading = false;
    render();
  }
}

async function apply(): Promise<void> {
  const sources = [...state.selected].filter((source) => sourceCount(source) > 0);
  if (sources.length === 0) return;

  state.applying = true;
  state.cancelRequested = false;
  state.progress = 0;
  state.progressMessage = "Preparing transaction…";
  state.operationTitle = "Updating the system";
  state.log = [];
  state.error = null;
  state.notice = null;
  render();

  try {
    const outcome: ApplyOutcome = await applyUpdates(sources, []);
    state.applying = false;
    if (outcome.cancelled) {
      state.notice = "Update cancelled.";
    } else if (!outcome.success) {
      state.error = outcome.error ?? "The update did not complete.";
    } else {
      state.notice = "Update complete.";
    }
  } catch (error) {
    state.applying = false;
    state.error = String(error);
  }

  render();
  if (!state.error) {
    await refresh();
  }
}

async function cancel(): Promise<void> {
  if (state.cancelRequested) return;
  state.cancelRequested = true;
  state.progressMessage = "Cancelling…";
  render();
  try {
    await cancelUpdates();
  } catch (error) {
    state.error = String(error);
    render();
  }
}

async function runSearch(): Promise<void> {
  state.searchLoading = true;
  render();
  try {
    const results = await searchPackages(state.searchQuery);
    state.searchResults = results;
    const keep =
      state.selectedPackage !== null &&
      results.some((item) => item.name === state.selectedPackage);
    if (!keep) {
      state.selectedPackage = results[0]?.name ?? null;
      state.packageDetails = null;
      if (state.selectedPackage) {
        try {
          state.packageDetails = await packageInfo(state.selectedPackage);
        } catch (error) {
          state.error = String(error);
        }
      }
    }
  } catch (error) {
    state.error = String(error);
  } finally {
    state.searchLoading = false;
    render();
  }
}

async function loadPackage(name: string): Promise<void> {
  state.selectedPackage = name;
  state.packageDetails = null;
  render();
  try {
    state.packageDetails = await packageInfo(name);
  } catch (error) {
    state.error = String(error);
  }
  render();
}

async function openPreview(action: PackageAction): Promise<void> {
  const name = state.selectedPackage;
  if (!name) return;
  state.packageBusy = true;
  state.error = null;
  state.notice = null;
  render();
  try {
    state.packagePreview = await previewPackages(action, [name]);
  } catch (error) {
    state.error = String(error);
  } finally {
    state.packageBusy = false;
    render();
  }
}

async function confirmPreview(): Promise<void> {
  const preview = state.packagePreview;
  const name = state.selectedPackage;
  if (!preview || !name) return;

  state.packagePreview = null;
  state.applying = true;
  state.cancelRequested = false;
  state.progress = 0;
  state.progressMessage = "Preparing transaction…";
  state.operationTitle =
    preview.action === "install" ? "Installing packages" : "Removing packages";
  state.log = [];
  state.error = null;
  state.notice = null;
  render();

  try {
    const outcome = await applyPackages(preview.action, [name]);
    state.applying = false;
    if (outcome.cancelled) {
      state.notice = `${packageActionLabel(preview.action)} cancelled.`;
    } else if (!outcome.success) {
      state.error = outcome.error ?? "The transaction did not complete.";
    } else {
      state.notice =
        (preview.action === "install" ? "Install complete." : "Removal complete.") +
        (outcome.reboot_suggested ? " Reboot suggested." : "");
    }
  } catch (error) {
    state.applying = false;
    state.error = String(error);
  }

  render();
  if (!state.error) {
    await runSearch();
    if (state.selectedPackage) await loadPackage(state.selectedPackage);
  }
}

async function openSettings(): Promise<void> {
  state.settingsOpen = true;
  render();
  if (state.repositoryStatus !== null) return;
  try {
    state.repositoryStatus = await loadRepositoryStatus();
  } catch (error) {
    state.error = String(error);
  }
  render();
}

async function toggleRepository(name: string, enabled: boolean): Promise<void> {
  if (state.repositoryBusy) return;
  state.repositoryBusy = true;
  state.error = null;
  state.notice = null;
  render();
  try {
    state.repositoryStatus = await setRepository(name, enabled);
    state.notice = enabled ? `${name} enabled.` : `${name} disabled.`;
  } catch (error) {
    state.error = String(error);
  } finally {
    state.repositoryBusy = false;
    render();
  }
}

async function applyParallelDownloads(): Promise<void> {
  const field = document.querySelector<HTMLInputElement>("#setting-parallel");
  if (!field || state.repositoryBusy) return;
  state.repositoryBusy = true;
  state.error = null;
  state.notice = null;
  render();
  try {
    state.repositoryStatus = await setPacmanOption("ParallelDownloads", field.value);
    state.notice = "Parallel downloads updated.";
  } catch (error) {
    state.error = String(error);
  } finally {
    state.repositoryBusy = false;
    render();
  }
}

async function subscribeToUpdates(): Promise<void> {
  if (!isTauri) return;
  const { listen } = await import("@tauri-apps/api/event");
  await listen<{ percent: number; message: string }>("skemer://update-progress", (event) => {
    const percent = Number(event.payload.percent);
    const next = Number.isFinite(percent) ? Math.max(0, Math.min(100, percent)) : 0;
    state.progress = Math.max(state.progress, next);
    state.progressMessage = event.payload.message;
    scheduleRender();
  });
  await listen<{ line: string }>("skemer://update-log", (event) => {
    state.log.push(event.payload.line);
    if (state.log.length > 400) {
      state.log.splice(0, state.log.length - 400);
    }
    scheduleRender();
  });
}

async function saveSettingsFromForm(): Promise<void> {
  const interval = document.querySelector<HTMLSelectElement>("#setting-interval");
  const notifications = document.querySelector<HTMLInputElement>("#setting-notifications");
  const autostart = document.querySelector<HTMLInputElement>("#setting-autostart");
  const closeToTray = document.querySelector<HTMLInputElement>("#setting-close-to-tray");
  const flatpakSystem = document.querySelector<HTMLInputElement>("#setting-flatpak-system");
  const flatpakUser = document.querySelector<HTMLInputElement>("#setting-flatpak-user");

  state.settings = {
    check_interval_hours: Number(interval?.value ?? 6),
    notifications: Boolean(notifications?.checked),
    autostart: Boolean(autostart?.checked),
    close_to_tray: Boolean(closeToTray?.checked),
    include_flatpak_system: Boolean(flatpakSystem?.checked),
    include_flatpak_user: Boolean(flatpakUser?.checked),
  };

  try {
    await saveSettings(state.settings);
    applySettingsToSelection(state.settings);
    scheduleAutoRefresh();
    state.settingsOpen = false;
    await refresh();
  } catch (error) {
    state.error = String(error);
  }
  render();
}

void (async () => {
  try {
    state.settings = await loadSettings();
  } catch (error) {
    state.error = String(error);
  }
  applySettingsToSelection(state.settings);
  scheduleAutoRefresh();
  try {
    await subscribeToUpdates();
  } catch (error) {
    state.error = String(error);
  }
  render();
  await refresh();
})();
