use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use alpm::{Alpm, SigLevel, TransFlag};
use anyhow::{anyhow, Context, Result};
use skemer_core::models::{
    ApplyOutcome, ApplyRequest, HoldDecision, PackageAction, PackageDetails, PackageHit,
    PackageOutcome, PackagePreview, PackagePreviewItem, PackageQuery, PackageRequest,
    RepositoryInfo, RepositoryStatus, UpdateItem, UpdatePlan, UpdateSource,
};
use skemer_core::sources::FlatpakScope;
use skemer_core::{packages, pacman_conf, pacman_config, plan, CancelHandle};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

const ROOT: &str = "/";
const DB_PATH: &str = "/var/lib/pacman";
const SYNC_PATH: &str = "/var/lib/pacman/sync";
const PACMAN_CONF: &str = "/etc/pacman.conf";

/// Sync databases are metadata, not a transaction. Coalesce rapid checks so a
/// local user cannot turn every check into a mirror download.
const DATABASE_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
static LAST_DATABASE_REFRESH: Mutex<Option<Instant>> = Mutex::new(None);

/// Read the pending system update plan from the pacman databases.
///
/// `pacman -Syu` refreshes the sync databases before it plans an upgrade.
/// Checking only the cached databases misses updates until something else runs
/// `-Sy`, so refresh first and fall back to the cache when the refresh fails.
pub fn check() -> Result<UpdatePlan> {
    let mut warnings = Vec::new();
    if let Err(error) = refresh_databases_if_due() {
        warnings.push(format!("Could not refresh the package databases: {error}"));
    }

    let mut handle = SystemHandle::open()?;
    let items = handle.update_items();
    let items = match items {
        Ok(items) => items,
        Err(error) => {
            // A read-only check does not need the pacman lock. If another
            // process holds it, fall back quietly instead of alarming the user.
            if error.downcast_ref::<alpm::Error>() != Some(&alpm::Error::HandleLock) {
                handle
                    .warnings
                    .push(format!("Could not build the pacman transaction: {error}"));
            }
            handle.scan_items()?
        }
    };

    let reboot_suggested = plan::reboot_suggested_now(&items);
    warnings.append(&mut handle.warnings);
    let mut plan = UpdatePlan::from_items(items, now());
    plan.reboot_suggested = reboot_suggested;
    plan.warnings = warnings;
    Ok(plan)
}

fn refresh_databases_if_due() -> Result<()> {
    let now = Instant::now();
    {
        let last = LAST_DATABASE_REFRESH
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !refresh_due(*last, now) {
            return Ok(());
        }
    }

    refresh_databases_with("pacman")?;

    let mut last = LAST_DATABASE_REFRESH
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *last = Some(now);
    Ok(())
}

fn refresh_due(last: Option<Instant>, now: Instant) -> bool {
    last.map(|last| now.saturating_duration_since(last) >= DATABASE_REFRESH_INTERVAL)
        .unwrap_or(true)
}

fn refresh_databases_with(pacman: &str) -> Result<()> {
    let output = StdCommand::new(pacman)
        .args(["-Sy", "--noconfirm", "--color", "never"])
        .env("LC_ALL", "C")
        .output()
        .with_context(|| format!("run {pacman} -Sy"))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    if stderr.is_empty() {
        anyhow::bail!("{pacman} -Sy exited with {}", output.status);
    }
    anyhow::bail!("{stderr}");
}

/// Validate a proposed hold set with libalpm. A hold is only offered when the
/// remaining transaction can still be prepared.
pub fn prepare(holds: &[String]) -> Result<HoldDecision> {
    let mut handle = SystemHandle::open()?;

    if !holds.is_empty() {
        // If the plan cannot be read (for example because another process holds
        // the pacman lock), let the prepare step report the real error instead
        // of rejecting every hold as unknown.
        if let Ok(items) = handle.update_items() {
            let pending: HashSet<String> = items.into_iter().map(|item| item.name).collect();
            let unknown: Vec<String> = holds
                .iter()
                .filter(|hold| !pending.contains(*hold))
                .cloned()
                .collect();
            if !unknown.is_empty() {
                return Ok(HoldDecision::rejected(
                    format!(
                        "These packages are not pending updates: {}",
                        unknown.join(", ")
                    ),
                    unknown,
                ));
            }
        }
    }

    handle.prepare_with(holds)
}

/// Progress and log events from a running apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyEvent {
    Progress { percent: u32, message: String },
    Log(String),
}

struct ChildOutcome {
    success: bool,
    cancelled: bool,
    percent: u32,
    log: String,
    error: Option<String>,
}

/// Run a child process, forwarding each line as a log event and letting the
/// caller turn selected lines into progress events.
async fn run_child<F>(
    mut command: tokio::process::Command,
    label: &str,
    start_percent: u32,
    mut classify: F,
    events: mpsc::Sender<ApplyEvent>,
    cancel: CancelHandle,
) -> Result<ChildOutcome>
where
    F: FnMut(&str, u32) -> Option<(u32, String)>,
{
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);

    let mut attempts = 0;
    let mut child = loop {
        match command.spawn() {
            Ok(child) => break child,
            Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) && attempts < 5 => {
                // A freshly written test helper (or a pacman binary being
                // replaced) can be briefly busy. Retry for a short moment.
                attempts += 1;
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            Err(error) => return Err(error).with_context(|| format!("run {label}")),
        }
    };
    cancel.set_pid(child.id());
    if cancel.is_cancelled() {
        cancel.cancel();
    }

    let (line_tx, mut line_rx) = mpsc::channel::<String>(256);
    if let Some(stdout) = child.stdout.take() {
        let sender = line_tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if sender.send(line).await.is_err() {
                    break;
                }
            }
        });
    }
    if let Some(stderr) = child.stderr.take() {
        let sender = line_tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if sender.send(line).await.is_err() {
                    break;
                }
            }
        });
    }
    drop(line_tx);

    let wait = tokio::spawn(async move { child.wait().await });

    let mut log_tail: VecDeque<String> = VecDeque::with_capacity(200);
    let mut percent = start_percent;
    while let Some(line) = line_rx.recv().await {
        let line = line.trim_end().to_string();
        if line.is_empty() {
            continue;
        }
        let _ = events.send(ApplyEvent::Log(line.clone())).await;

        if let Some((new_percent, message)) = classify(&line, percent) {
            percent = new_percent;
            let _ = events.send(ApplyEvent::Progress { percent, message }).await;
        }

        if log_tail.len() == 200 {
            log_tail.pop_front();
        }
        log_tail.push_back(line);
    }

    let status = wait.await.context("join the child wait task")??;
    let cancelled = cancel.is_cancelled() && !status.success();
    cancel.set_pid(None);

    let log = log_tail.into_iter().collect::<Vec<_>>().join("\n");
    let success = status.success() && !cancelled;
    let error = if success {
        None
    } else if cancelled {
        Some("Update cancelled.".to_string())
    } else {
        Some(plan::failure_message(&log))
    };

    Ok(ChildOutcome {
        success,
        cancelled,
        percent,
        log,
        error,
    })
}

fn pacman_progress(line: &str, percent: u32) -> Option<(u32, String)> {
    if let Some((new_percent, message)) = plan::parse_progress(line) {
        Some((new_percent, message))
    } else {
        line.strip_prefix(":: ")
            .map(|message| (percent, message.to_string()))
    }
}

async fn apply_system(
    request: &ApplyRequest,
    reboot_suggested: bool,
    pacman: &str,
    events: mpsc::Sender<ApplyEvent>,
    cancel: CancelHandle,
) -> Result<ApplyOutcome> {
    let holds = normalized_holds(&request.holds);
    emit(
        &events,
        ApplyEvent::Progress {
            percent: 0,
            message: "Preparing transaction".to_string(),
        },
    )
    .await;

    let mut command = tokio::process::Command::new(pacman);
    command
        .arg("-Syu")
        .arg("--noconfirm")
        .arg("--color")
        .arg("never")
        .env("LC_ALL", "C");
    if !holds.is_empty() {
        command.arg("--ignore").arg(holds.join(","));
    }

    let outcome = run_child(
        command,
        &format!("{pacman} -Syu"),
        0,
        pacman_progress,
        events.clone(),
        cancel,
    )
    .await?;

    let candidates = if outcome.success || outcome.cancelled {
        Vec::new()
    } else {
        plan::hold_candidates_from_error(&outcome.log)
    };
    let message = if outcome.success {
        "System updated"
    } else if outcome.cancelled {
        "Update cancelled"
    } else {
        "Update failed"
    };
    let _ = events
        .send(ApplyEvent::Progress {
            percent: if outcome.success { 90 } else { outcome.percent },
            message: message.to_string(),
        })
        .await;

    let error = plan::command_error(outcome.error, &outcome.log);
    Ok(ApplyOutcome {
        success: outcome.success,
        cancelled: outcome.cancelled,
        held_packages: holds,
        reboot_suggested,
        log_tail: outcome.log,
        error,
        candidates,
    })
}

async fn apply_flatpak(
    scope: FlatpakScope,
    flatpak: &str,
    start_percent: u32,
    events: mpsc::Sender<ApplyEvent>,
    cancel: CancelHandle,
) -> Result<ApplyOutcome> {
    let label = scope.source().label();
    let mut command = tokio::process::Command::new(flatpak);
    command
        .arg(scope.flag())
        .arg("update")
        .arg("-y")
        .arg("--noninteractive")
        .env("LC_ALL", "C");

    let start = start_percent;
    let outcome = run_child(
        command,
        &format!("{flatpak} {} update", scope.flag()),
        start_percent,
        move |line: &str, percent| {
            if line.starts_with("Updating ") || line.starts_with("Looking for updates") {
                Some((percent.max(start), line.trim().to_string()))
            } else {
                None
            }
        },
        events.clone(),
        cancel,
    )
    .await?;

    let message = if outcome.success {
        format!("{label} updated")
    } else if outcome.cancelled {
        "Update cancelled".to_string()
    } else {
        "Update failed".to_string()
    };
    let _ = events
        .send(ApplyEvent::Progress {
            percent: if outcome.success { 95 } else { outcome.percent },
            message,
        })
        .await;

    Ok(ApplyOutcome {
        success: outcome.success,
        cancelled: outcome.cancelled,
        held_packages: Vec::new(),
        reboot_suggested: false,
        log_tail: outcome.log,
        error: outcome.error,
        candidates: Vec::new(),
    })
}

/// Run the selected helper-side sources: the system, then Flatpak system.
/// Flatpak user updates run in the user session and are rejected here.
pub async fn apply_streaming(
    request: &ApplyRequest,
    reboot_suggested: bool,
    pacman: &str,
    flatpak: &str,
    events: mpsc::Sender<ApplyEvent>,
    cancel: CancelHandle,
) -> Result<ApplyOutcome> {
    if request.sources.contains(&UpdateSource::FlatpakUser) {
        anyhow::bail!("Flatpak user updates run in the user session");
    }
    if request.sources.is_empty() {
        return Ok(empty_outcome());
    }

    let system_selected = request.sources.contains(&UpdateSource::System);
    let flatpak_selected = request.sources.contains(&UpdateSource::FlatpakSystem);
    let mut log = String::new();
    let mut held_packages = Vec::new();
    let mut reboot_suggested = reboot_suggested;

    if system_selected {
        let outcome = apply_system(
            request,
            reboot_suggested,
            pacman,
            events.clone(),
            cancel.clone(),
        )
        .await?;
        if !outcome.success {
            return Ok(outcome);
        }
        log = outcome.log_tail;
        held_packages = outcome.held_packages;
        reboot_suggested = outcome.reboot_suggested;
    }

    if flatpak_selected {
        let start = if system_selected { 90 } else { 50 };
        let outcome =
            apply_flatpak(FlatpakScope::System, flatpak, start, events.clone(), cancel).await?;
        log = combine_logs(log, outcome.log_tail);
        if !outcome.success {
            return Ok(ApplyOutcome {
                success: false,
                cancelled: outcome.cancelled,
                held_packages,
                reboot_suggested,
                log_tail: log,
                error: outcome.error,
                candidates: Vec::new(),
            });
        }
    }

    let _ = events
        .send(ApplyEvent::Progress {
            percent: 100,
            message: "Update complete".to_string(),
        })
        .await;

    Ok(ApplyOutcome {
        success: true,
        cancelled: false,
        held_packages,
        reboot_suggested,
        log_tail: log,
        error: None,
        candidates: Vec::new(),
    })
}

async fn emit(events: &mpsc::Sender<ApplyEvent>, event: ApplyEvent) {
    let _ = events.send(event).await;
}

fn combine_logs(first: String, second: String) -> String {
    if first.is_empty() {
        second
    } else if second.is_empty() {
        first
    } else {
        format!("{first}\n{second}")
    }
}

fn empty_outcome() -> ApplyOutcome {
    ApplyOutcome {
        success: true,
        cancelled: false,
        held_packages: Vec::new(),
        reboot_suggested: false,
        log_tail: String::new(),
        error: None,
        candidates: Vec::new(),
    }
}

struct SystemHandle {
    alpm: Alpm,
    warnings: Vec<String>,
}

impl SystemHandle {
    fn open() -> Result<Self> {
        let conf = pacman_conf::load(Path::new(PACMAN_CONF)).unwrap_or_default();
        let mut alpm = Alpm::new(ROOT, DB_PATH).context("open the pacman database")?;

        if !conf.ignore_pkgs.is_empty() {
            alpm.set_ignorepkgs(conf.ignore_pkgs.iter().map(String::as_str))
                .context("apply IgnorePkg from pacman.conf")?;
        }
        if !conf.ignore_groups.is_empty() {
            alpm.set_ignoregroups(conf.ignore_groups.iter().map(String::as_str))
                .context("apply IgnoreGroup from pacman.conf")?;
        }

        let mut warnings = Vec::new();
        let repositories = if conf.repositories.is_empty() {
            scan_sync_repositories()
        } else {
            conf.repositories
        };

        for repository in repositories {
            let database = sync_database(&repository);
            if !database.exists() {
                warnings.push(format!(
                    "The package database for {repository} is missing. Run pacman -Sy."
                ));
                continue;
            }
            alpm.register_syncdb(repository.as_str(), SigLevel::USE_DEFAULT)
                .with_context(|| format!("register the {repository} package database"))?;
        }

        if alpm.syncdbs().is_empty() {
            warnings.push("No package databases were found. Run pacman -Sy.".to_string());
        }

        Ok(Self { alpm, warnings })
    }

    /// Build the update list from the transaction pacman itself would run. This
    /// includes replacements, not just same-name upgrades.
    fn update_items(&mut self) -> Result<Vec<UpdateItem>> {
        self.alpm
            .trans_init(TransFlag::NONE)
            .context("start the libalpm transaction")?;

        let result = (|| -> Result<(Vec<UpdateItem>, Vec<String>)> {
            self.alpm
                .sync_sysupgrade(false)
                .context("compute the sysupgrade")?;
            let mut items = Vec::new();
            for package in self.alpm.trans_add().iter() {
                items.push(item_for(&self.alpm, package));
            }
            items.sort_by(|left, right| left.name.cmp(&right.name));
            let removals = self
                .alpm
                .trans_remove()
                .iter()
                .map(|package| package.name().to_string())
                .collect();
            Ok((items, removals))
        })();

        // Release even when the transaction could not be built, otherwise the
        // pacman lock is left behind.
        let release = self.alpm.trans_release();
        let (items, removals) = result?;
        release.context("release the libalpm transaction")?;

        if !removals.is_empty() {
            self.warnings
                .push(format!("This update removes: {}.", removals.join(", ")));
        }

        Ok(items)
    }

    /// Fallback when the transaction cannot be built: compare local packages
    /// against the sync databases one by one.
    fn scan_items(&self) -> Result<Vec<UpdateItem>> {
        let mut items = Vec::new();
        for package in self.alpm.localdb().pkgs().iter() {
            let Some(candidate) = package.sync_new_version(self.alpm.syncdbs()) else {
                continue;
            };
            items.push(item_for(&self.alpm, candidate));
        }
        items.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(items)
    }

    fn prepare_with(&mut self, holds: &[String]) -> Result<HoldDecision> {
        for hold in holds {
            self.alpm
                .add_ignorepkg(hold.as_str())
                .with_context(|| format!("hold {hold} for this transaction"))?;
        }

        self.alpm
            .trans_init(TransFlag::NONE)
            .context("start the libalpm transaction")?;

        let result = self
            .alpm
            .sync_sysupgrade(false)
            .map_err(|error| error.to_string())
            .and_then(|()| self.alpm.trans_prepare().map_err(|error| error.to_string()));

        let release = self.alpm.trans_release();

        match result {
            Ok(()) => {
                release.context("release the libalpm transaction")?;
                let reason = if holds.is_empty() {
                    "The full transaction is valid.".to_string()
                } else {
                    format!("The transaction is valid with {} held.", holds.join(", "))
                };
                Ok(HoldDecision::safe(reason))
            }
            Err(error) => {
                let _ = release;
                Ok(HoldDecision::rejected(
                    error.clone(),
                    plan::hold_candidates_from_error(&error),
                ))
            }
        }
    }
}

fn item_for(alpm: &Alpm, package: &alpm::Package) -> UpdateItem {
    let name = package.name().to_string();
    let current_version = alpm
        .localdb()
        .pkg(name.as_str())
        .map(|local| local.version().to_string())
        .unwrap_or_else(|_| "not installed".to_string());
    let repo = package
        .db()
        .map(|database| database.name().to_string())
        .unwrap_or_default();

    UpdateItem {
        id: format!("system:{name}"),
        source: UpdateSource::System,
        name: name.clone(),
        repo,
        current_version,
        new_version: package.version().to_string(),
        download_bytes: package.size().max(0) as u64,
        installed_bytes: package.isize().max(0) as u64,
        summary: package.desc().unwrap_or_default().to_string(),
        reboot_suggested: plan::is_reboot_package(&name),
        warning: None,
    }
}

fn scan_sync_repositories() -> Vec<String> {
    let mut repositories = Vec::new();
    let Ok(entries) = fs::read_dir(SYNC_PATH) else {
        return repositories;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("db") {
            continue;
        }
        if let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) {
            repositories.push(name.to_string());
        }
    }
    repositories.sort();
    repositories
}

fn sync_database(repository: &str) -> PathBuf {
    Path::new(SYNC_PATH).join(format!("{repository}.db"))
}

fn normalized_holds(holds: &[String]) -> Vec<String> {
    let mut holds: Vec<String> = holds
        .iter()
        .map(|hold| hold.trim().to_string())
        .filter(|hold| !hold.is_empty())
        .collect();
    holds.sort();
    holds.dedup();
    holds
}

fn now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Search enabled sync databases. The first repository that defines a name wins,
/// matching pacman's own preference order.
pub fn search(request: &PackageQuery) -> Result<Vec<PackageHit>> {
    let handle = SystemHandle::open()?;
    let needle = request.query.trim().to_ascii_lowercase();
    let limit = packages::clamp_limit(request.limit);
    let mut hits: HashMap<String, PackageHit> = HashMap::new();

    for database in handle.alpm.syncdbs().iter() {
        let repo = database.name().to_string();
        if let Some(filter) = &request.repo {
            if !filter.eq_ignore_ascii_case(&repo) {
                continue;
            }
        }
        for package in database.pkgs().iter() {
            let name = package.name().to_string();
            let summary = package.desc().unwrap_or_default().to_string();
            if !needle.is_empty()
                && !name.to_ascii_lowercase().contains(&needle)
                && !summary.to_ascii_lowercase().contains(&needle)
            {
                continue;
            }

            let installed_version = handle
                .alpm
                .localdb()
                .pkg(name.as_str())
                .ok()
                .map(|local| local.version().to_string());
            if request.installed_only && installed_version.is_none() {
                continue;
            }

            hits.entry(name.clone()).or_insert_with(|| PackageHit {
                name,
                repo: repo.clone(),
                version: package.version().to_string(),
                installed_version,
                download_bytes: package.size().max(0) as u64,
                installed_bytes: package.isize().max(0) as u64,
                summary,
                groups: package.groups().iter().map(ToString::to_string).collect(),
            });
        }
    }

    let mut hits: Vec<PackageHit> = hits.into_values().collect();
    hits.sort_by(|left, right| {
        let left_rank = packages::search_rank(&needle, &left.name).unwrap_or(u8::MAX);
        let right_rank = packages::search_rank(&needle, &right.name).unwrap_or(u8::MAX);
        left_rank
            .cmp(&right_rank)
            .then_with(|| left.name.cmp(&right.name))
    });
    hits.truncate(limit);
    Ok(hits)
}

/// Read one package from a sync database, falling back to the local database.
pub fn details(name: &str) -> Result<PackageDetails> {
    let handle = SystemHandle::open()?;
    let sync = handle.alpm.syncdbs().iter().find_map(|database| {
        database
            .pkg(name)
            .ok()
            .map(|package| (database.name().to_string(), package))
    });
    let local = handle.alpm.localdb().pkg(name).ok();

    let (repo, package, installed_version) = match sync {
        Some((repo, package)) => (
            Some(repo),
            package,
            local.map(|local| local.version().to_string()),
        ),
        None => match local {
            Some(package) => (None, package, Some(package.version().to_string())),
            None => anyhow::bail!("{name} was not found in the enabled repositories"),
        },
    };

    let mut required_by = Vec::new();
    if installed_version.is_some() {
        for installed in handle.alpm.localdb().pkgs().iter() {
            if installed.name() == name {
                continue;
            }
            if installed
                .depends()
                .iter()
                .any(|dependency| dependency.name() == name)
            {
                required_by.push(installed.name().to_string());
            }
        }
        required_by.sort();
    }

    Ok(PackageDetails {
        name: package.name().to_string(),
        repo,
        version: package.version().to_string(),
        installed_version,
        description: package.desc().unwrap_or_default().to_string(),
        url: package.url().unwrap_or_default().to_string(),
        packager: package.packager().unwrap_or_default().to_string(),
        build_date: package.build_date(),
        download_bytes: package.size().max(0) as u64,
        installed_bytes: package.isize().max(0) as u64,
        licenses: package.licenses().iter().map(ToString::to_string).collect(),
        groups: package.groups().iter().map(ToString::to_string).collect(),
        depends: package.depends().iter().map(ToString::to_string).collect(),
        optdepends: package
            .optdepends()
            .iter()
            .map(ToString::to_string)
            .collect(),
        conflicts: package
            .conflicts()
            .iter()
            .map(ToString::to_string)
            .collect(),
        replaces: package.replaces().iter().map(ToString::to_string).collect(),
        provides: package.provides().iter().map(ToString::to_string).collect(),
        required_by,
    })
}

/// Resolve a proposed install or removal with libalpm without touching the
/// system. This is the same read-only preparation used for update holds.
pub fn preview(request: &PackageRequest) -> Result<PackagePreview> {
    packages::validate_names(&request.names).map_err(anyhow::Error::msg)?;

    let mut handle = SystemHandle::open()?;
    handle
        .alpm
        .trans_init(TransFlag::NONE)
        .map_err(|error| match error {
            alpm::Error::HandleLock => anyhow!(
                "Another package manager is using the pacman database. Try again when it finishes."
            ),
            other => anyhow!(other).context("start the libalpm transaction"),
        })?;

    let result = (|| -> Result<PackagePreview> {
        let mut pending_system_updates = 0;
        let mut warnings = Vec::new();

        match request.action {
            PackageAction::Install => {
                handle
                    .alpm
                    .sync_sysupgrade(false)
                    .context("compute the sysupgrade")?;
                pending_system_updates = handle.alpm.trans_add().iter().count();
                let pending: HashSet<String> = handle
                    .alpm
                    .trans_add()
                    .iter()
                    .map(|package| package.name().to_string())
                    .collect();

                for name in &request.names {
                    for package in resolve_sync_targets(&handle.alpm, name)? {
                        if let Ok(local) = handle.alpm.localdb().pkg(package.name()) {
                            if local.version().as_str() == package.version().as_str() {
                                continue;
                            }
                        }
                        if pending.contains(package.name()) {
                            continue;
                        }
                        handle
                            .alpm
                            .trans_add_pkg(package)
                            .map_err(|error| anyhow!(error.to_string()))?;
                    }
                }
            }
            PackageAction::Remove => {
                for name in &request.names {
                    let package = handle.alpm.localdb().pkg(name.as_str()).with_context(|| {
                        format!("{name} is not installed, so it cannot be removed")
                    })?;
                    handle
                        .alpm
                        .trans_remove_pkg(package)
                        .map_err(|error| anyhow!(error.to_string()))?;
                }
            }
        }

        handle
            .alpm
            .trans_prepare()
            .map_err(|error| anyhow!(prepare_error_message(&error)))?;

        let adds: Vec<PackagePreviewItem> = handle
            .alpm
            .trans_add()
            .iter()
            .map(|package| preview_item(&handle.alpm, package))
            .collect();
        let removes: Vec<PackagePreviewItem> = handle
            .alpm
            .trans_remove()
            .iter()
            .map(|package| preview_item(&handle.alpm, package))
            .collect();

        if pending_system_updates > 0 && request.action == PackageAction::Install {
            warnings.push(format!(
                "This transaction also applies {pending_system_updates} pending system update(s)."
            ));
        }
        if !removes.is_empty() {
            warnings.push(format!(
                "This transaction removes: {}.",
                removes
                    .iter()
                    .map(|item| item.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }

        let removed: HashSet<&str> = removes.iter().map(|item| item.name.as_str()).collect();
        for package in handle.alpm.trans_add().iter() {
            for replacement in package.replaces().iter() {
                if removed.contains(replacement.name()) {
                    warnings.push(format!(
                        "{} replaces {}.",
                        package.name(),
                        replacement.name()
                    ));
                }
            }
        }

        let total_download_bytes = adds.iter().map(|item| item.download_bytes).sum();
        let total_installed_bytes = if request.action == PackageAction::Remove {
            removes.iter().map(|item| item.installed_bytes).sum()
        } else {
            adds.iter().map(|item| item.installed_bytes).sum()
        };
        let reboot_suggested = adds.iter().any(|item| plan::is_reboot_package(&item.name));

        Ok(PackagePreview {
            action: request.action,
            adds,
            removes,
            total_download_bytes,
            total_installed_bytes,
            pending_system_updates,
            reboot_suggested,
            warnings,
        })
    })();

    let release = handle.alpm.trans_release();
    let preview = result?;
    release.context("release the libalpm transaction")?;
    Ok(preview)
}

/// Turn libalpm's prepare failure into something the user can act on. The
/// error kind alone ("could not satisfy dependencies") does not say which
/// package blocks a removal.
fn prepare_error_message(error: &alpm::PrepareError<'_>) -> String {
    let base = error.error().to_string();
    match error.data() {
        Some(alpm::PrepareData::UnsatisfiedDeps(missing)) => {
            let details: Vec<String> = missing
                .iter()
                .map(|item| {
                    let dependency = item.depend().to_string();
                    match item.causing_pkg() {
                        Some(causing) => format!(
                            "{} requires {}, but {} would be removed",
                            item.target(),
                            dependency,
                            causing
                        ),
                        None => format!("{} requires {}", item.target(), dependency),
                    }
                })
                .collect();
            if details.is_empty() {
                base
            } else {
                format!("{base}: {}", details.join("; "))
            }
        }
        Some(alpm::PrepareData::ConflictingDeps(conflicts)) => {
            let details: Vec<String> = conflicts
                .iter()
                .map(|conflict| {
                    format!(
                        "{} conflicts with {} ({})",
                        conflict.package1().name(),
                        conflict.package2().name(),
                        conflict.reason()
                    )
                })
                .collect();
            if details.is_empty() {
                base
            } else {
                format!("{base}: {}", details.join("; "))
            }
        }
        Some(alpm::PrepareData::PkgInvalidArch(packages)) => {
            let names: Vec<&str> = packages.iter().map(|package| package.name()).collect();
            if names.is_empty() {
                base
            } else {
                format!("{base}: {}", names.join(", "))
            }
        }
        None => base,
    }
}

fn resolve_sync_targets<'a>(alpm: &'a Alpm, name: &str) -> Result<Vec<&'a alpm::Package>> {
    for database in alpm.syncdbs().iter() {
        if let Ok(package) = database.pkg(name) {
            return Ok(vec![package]);
        }
    }
    for database in alpm.syncdbs().iter() {
        if let Ok(group) = database.group(name) {
            let packages: Vec<&alpm::Package> = group.packages().iter().collect();
            if !packages.is_empty() {
                return Ok(packages);
            }
        }
    }
    anyhow::bail!("{name} was not found in the enabled repositories")
}

fn preview_item(alpm: &Alpm, package: &alpm::Package) -> PackagePreviewItem {
    let current_version = alpm
        .localdb()
        .pkg(package.name())
        .ok()
        .map(|local| local.version().to_string());
    PackagePreviewItem {
        name: package.name().to_string(),
        repo: package.db().map(|database| database.name().to_string()),
        version: package.version().to_string(),
        current_version,
        download_bytes: package.size().max(0) as u64,
        installed_bytes: package.isize().max(0) as u64,
    }
}

/// The exact pacman arguments for a package transaction. Kept separate so the
/// partial-upgrade rule is testable without touching a database.
fn package_args(action: PackageAction, names: &[String]) -> Vec<String> {
    let mut args: Vec<String> = match action {
        PackageAction::Install => ["-Syu", "--needed", "--noconfirm", "--color", "never"]
            .into_iter()
            .map(ToString::to_string)
            .collect(),
        PackageAction::Remove => ["-Rns", "--noconfirm"]
            .into_iter()
            .map(ToString::to_string)
            .collect(),
    };
    args.extend(names.iter().cloned());
    args
}

/// Apply one install or removal transaction.
pub async fn apply_packages(
    request: &PackageRequest,
    events: mpsc::Sender<ApplyEvent>,
    cancel: CancelHandle,
) -> Result<PackageOutcome> {
    packages::validate_names(&request.names).map_err(anyhow::Error::msg)?;
    // Validate the full transaction before pacman starts. The UI previews the
    // same result; this protects direct D-Bus callers.
    let preview = preview(request)?;

    let names = request.names.clone();
    let mut command = tokio::process::Command::new("pacman");
    let args = package_args(request.action, &names);
    let label = match request.action {
        PackageAction::Install => format!("pacman -Syu --needed {}", names.join(" ")),
        PackageAction::Remove => format!("pacman -Rns {}", names.join(" ")),
    };
    command
        .args(&args)
        .env("LC_ALL", "C")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let reboot_suggested = preview.reboot_suggested;

    let outcome = run_child(command, &label, 0, pacman_progress, events, cancel).await?;

    let error = plan::command_error(outcome.error, &outcome.log);
    Ok(PackageOutcome {
        success: outcome.success,
        cancelled: outcome.cancelled,
        action: request.action,
        names,
        reboot_suggested,
        log_tail: outcome.log,
        error,
    })
}

/// Effective repository state, including which file defines each repository.
pub fn repository_status() -> Result<RepositoryStatus> {
    let view = pacman_config::inspect(Path::new(ROOT))?;
    let handle = SystemHandle::open()?;
    let mut entries = view.repositories.clone();
    if view.multilib().is_none() {
        entries.push(pacman_config::RepoEntry {
            name: "multilib".to_string(),
            enabled: false,
            source_file: view
                .commented_multilib_source
                .clone()
                .unwrap_or_else(|| view.main_file.clone()),
            line: 0,
        });
    }

    let mut repositories = Vec::new();
    for entry in entries {
        let database = handle
            .alpm
            .syncdbs()
            .iter()
            .find(|database| database.name() == entry.name);
        let package_count = database.map(|database| database.pkgs().len()).unwrap_or(0);
        let synced_at = fs::metadata(sync_database(&entry.name))
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .map(format_time);

        let managed_by_include = entry.source_file != view.main_file;
        let toggleable = entry.name == "multilib"
            && if entry.enabled {
                !managed_by_include
            } else {
                !managed_by_include || entry.source_file == view.main_file
            };
        let note = if entry.name == "multilib" {
            if entry.enabled && managed_by_include {
                Some(format!("Enabled by {}.", entry.source_file.display()))
            } else {
                None
            }
        } else if matches!(
            entry.name.as_str(),
            "core" | "extra" | "veldmuis-core" | "veldmuis-extra"
        ) {
            Some("Required repository.".to_string())
        } else {
            None
        };

        repositories.push(RepositoryInfo {
            name: entry.name,
            enabled: entry.enabled,
            source_file: entry.source_file.display().to_string(),
            note,
            toggleable,
            package_count,
            synced_at,
        });
    }

    let parallel_downloads_editable = view
        .parallel_downloads_source
        .as_deref()
        .map(|source| source == view.main_file.as_path())
        .unwrap_or(true);

    Ok(RepositoryStatus {
        pacman_conf: view.main_file.display().to_string(),
        repositories,
        warnings: view.warnings,
        parallel_downloads: view.parallel_downloads.unwrap_or(5),
        parallel_downloads_editable,
        parallel_downloads_source: view
            .parallel_downloads_source
            .map(|path| path.display().to_string()),
    })
}

/// Enable or disable multilib. Only `/etc/pacman.conf` is ever rewritten.
pub fn set_repository(name: &str, enabled: bool) -> Result<RepositoryStatus> {
    if name != "multilib" {
        anyhow::bail!("Only the multilib repository can be changed from Skemer");
    }
    let view = pacman_config::inspect(Path::new(ROOT))?;
    if view.pacnew {
        anyhow::bail!(
            "{} has a .pacnew file. Resolve the configuration changes before changing repositories.",
            view.main_file.display()
        );
    }

    let current = view.multilib();
    if enabled && current.is_some_and(|entry| entry.enabled) {
        return repository_status();
    }
    if !enabled && current.is_none() {
        return repository_status();
    }
    if !enabled {
        let entry = current.expect("checked above");
        if entry.source_file != view.main_file {
            anyhow::bail!(
                "multilib is managed by {} and cannot be disabled here",
                entry.source_file.display()
            );
        }
    }

    let original = fs::read_to_string(&view.main_file)
        .with_context(|| format!("read {}", view.main_file.display()))?;
    let updated = if enabled {
        pacman_config::enable_multilib_text(&original).map_err(anyhow::Error::msg)?
    } else {
        pacman_config::disable_multilib_text(&original).map_err(anyhow::Error::msg)?
    };
    if updated == original {
        return repository_status();
    }

    let backup = write_config_atomic(&view.main_file, &updated)?;
    if let Err(error) = sync_databases() {
        let _ = fs::copy(&backup, &view.main_file);
        return Err(error);
    }
    repository_status()
}

/// Set ParallelDownloads. Other pacman options stay a terminal concern.
pub fn set_parallel_downloads(value: u32) -> Result<RepositoryStatus> {
    if !(1..=10).contains(&value) {
        anyhow::bail!("Parallel downloads must be between 1 and 10");
    }
    let view = pacman_config::inspect(Path::new(ROOT))?;
    if view.pacnew {
        anyhow::bail!(
            "{} has a .pacnew file. Resolve the configuration changes before changing settings.",
            view.main_file.display()
        );
    }
    if let Some(source) = &view.parallel_downloads_source {
        if source != &view.main_file {
            anyhow::bail!(
                "ParallelDownloads is set by {} and cannot be changed here",
                source.display()
            );
        }
    }

    let original = fs::read_to_string(&view.main_file)
        .with_context(|| format!("read {}", view.main_file.display()))?;
    let updated =
        pacman_config::set_parallel_downloads_text(&original, value).map_err(anyhow::Error::msg)?;
    if updated == original {
        return repository_status();
    }

    write_config_atomic(&view.main_file, &updated)?;
    repository_status()
}

const SKEMER_STATE_DIR: &str = "/var/lib/skemer";

fn write_config_atomic(path: &Path, text: &str) -> Result<PathBuf> {
    let metadata =
        fs::metadata(path).with_context(|| format!("read metadata for {}", path.display()))?;
    let backup_dir = Path::new(SKEMER_STATE_DIR).join("backups");
    fs::create_dir_all(&backup_dir).with_context(|| format!("create {}", backup_dir.display()))?;
    let backup = backup_dir.join(format!(
        "pacman.conf.{}",
        chrono::Utc::now().timestamp_millis()
    ));
    fs::copy(path, &backup)
        .with_context(|| format!("back up {} to {}", path.display(), backup.display()))?;

    let temporary = path.with_file_name(format!(".pacman.conf.skemer-{}", std::process::id()));
    {
        let mut file = fs::File::create(&temporary)
            .with_context(|| format!("create {}", temporary.display()))?;
        file.write_all(text.as_bytes())
            .with_context(|| format!("write {}", temporary.display()))?;
        file.sync_all().ok();
    }
    fs::set_permissions(&temporary, metadata.permissions()).ok();
    if let Err(error) = validate_config(&temporary) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    fs::rename(&temporary, path).with_context(|| format!("replace {}", path.display()))?;
    Ok(backup)
}

fn validate_config(path: &Path) -> Result<()> {
    let output = StdCommand::new("pacman-conf")
        .arg("--config")
        .arg(path)
        .output()
        .context("run pacman-conf")?;
    if output.status.success() {
        Ok(())
    } else {
        anyhow::bail!(
            "pacman rejected the new configuration: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }
}

fn sync_databases() -> Result<()> {
    let output = StdCommand::new("pacman")
        .arg("-Sy")
        .arg("--noconfirm")
        .env("LC_ALL", "C")
        .output()
        .context("run pacman -Sy")?;
    if output.status.success() {
        Ok(())
    } else {
        anyhow::bail!(
            "the repository databases could not be synced: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }
}

fn format_time(time: std::time::SystemTime) -> String {
    let time: chrono::DateTime<chrono::Local> = time.into();
    time.format("%Y-%m-%d %H:%M").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    #[test]
    #[ignore = "reads the host pacman configuration"]
    fn reports_host_repositories() {
        let status = repository_status().expect("host repositories");
        assert!(status.repositories.iter().any(|repo| repo.name == "core"));
        assert!(status.parallel_downloads >= 1);
        eprintln!(
            "{} repositories, parallel downloads {}, multilib {:?}",
            status.repositories.len(),
            status.parallel_downloads,
            status
                .repositories
                .iter()
                .find(|repo| repo.name == "multilib")
        );
    }

    #[test]
    #[ignore = "reads the host pacman databases"]
    fn searches_the_host_packages() {
        let hits = search(&PackageQuery {
            query: "steam".to_string(),
            ..PackageQuery::default()
        })
        .expect("host search");
        assert!(hits.iter().any(|hit| hit.name == "steam"));

        let details = details("steam").expect("host details");
        assert_eq!(details.name, "steam");
        assert!(!details.version.is_empty());

        if is_root() {
            let preview = preview(&PackageRequest {
                action: PackageAction::Install,
                names: vec!["steam".to_string()],
            })
            .expect("host preview");
            assert_eq!(preview.action, PackageAction::Install);
        }
    }

    #[test]
    fn database_refresh_is_coalesced() {
        let now = Instant::now();
        assert!(refresh_due(None, now));
        assert!(!refresh_due(Some(now), now));
        assert!(!refresh_due(
            Some(now.checked_sub(Duration::from_secs(30)).expect("past")),
            now
        ));
        assert!(refresh_due(
            Some(now.checked_sub(Duration::from_secs(61)).expect("past")),
            now
        ));
    }

    #[test]
    fn database_refresh_uses_sync_only() {
        let marker =
            std::env::temp_dir().join(format!("skemer-refresh-args-{}", std::process::id()));
        let script = fake_program(
            "pacman-refresh",
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n",
                marker.display()
            ),
        );

        refresh_databases_with(script.to_str().expect("script path")).expect("refresh");
        let args = fs::read_to_string(&marker).expect("args");
        assert_eq!(
            args.lines().collect::<Vec<_>>(),
            vec!["-Sy", "--noconfirm", "--color", "never"]
        );
        fs::remove_file(&marker).ok();
    }

    #[test]
    fn database_refresh_reports_pacman_errors() {
        let script = fake_program(
            "pacman-refresh-fail",
            "#!/bin/sh\necho 'error: failed retrieving file' >&2\nexit 1\n",
        );

        let error = refresh_databases_with(script.to_str().expect("script path"))
            .expect_err("refresh failure");
        assert!(error.to_string().contains("failed retrieving file"));
    }

    #[test]
    fn install_command_folds_a_full_sync_in() {
        let names = vec!["steam".to_string(), "openai-codex".to_string()];
        let args = package_args(PackageAction::Install, &names);
        assert_eq!(
            args.iter().map(String::as_str).collect::<Vec<_>>(),
            vec![
                "-Syu",
                "--needed",
                "--noconfirm",
                "--color",
                "never",
                "steam",
                "openai-codex"
            ]
        );
        assert!(!args.iter().any(|argument| argument == "-S"));
    }

    #[test]
    fn remove_command_uses_the_removal_closure_flags() {
        let names = vec!["steam".to_string()];
        let args = package_args(PackageAction::Remove, &names);
        assert_eq!(
            args.iter().map(String::as_str).collect::<Vec<_>>(),
            vec!["-Rns", "--noconfirm", "steam"]
        );
    }

    #[test]
    fn holds_are_trimmed_sorted_and_deduplicated() {
        assert_eq!(
            normalized_holds(&[
                " linux ".to_string(),
                "mesa".to_string(),
                "linux".to_string(),
                "".to_string(),
            ]),
            vec!["linux".to_string(), "mesa".to_string()]
        );
    }

    #[test]
    fn failure_message_prefers_the_last_error_line() {
        let log = "warning: something\nsome error: first\nerror: failed to commit transaction";
        assert_eq!(
            plan::failure_message(log),
            "error: failed to commit transaction"
        );
    }

    #[tokio::test]
    async fn streaming_apply_reports_progress_and_logs() {
        let script = fake_pacman(
            "#!/bin/sh\n\
             echo ':: Starting full system upgrade...'\n\
             echo '(1/2) upgrading linux'\n\
             echo '(2/2) upgrading mesa'\n\
             exit 0\n",
        );
        let (tx, mut rx) = mpsc::channel(64);
        let request = request(&[UpdateSource::System], &[]);
        let outcome = apply_streaming(
            &request,
            true,
            script.to_str().expect("script path"),
            "/bin/true",
            tx,
            CancelHandle::default(),
        )
        .await
        .expect("apply");
        let events = drain(&mut rx).await;

        assert!(outcome.success);
        assert!(!outcome.cancelled);
        assert!(outcome.reboot_suggested);
        assert!(outcome.log_tail.contains("(2/2) upgrading mesa"));
        assert!(events
            .iter()
            .any(|event| matches!(event, ApplyEvent::Progress { percent: 50, .. })));
        assert!(events
            .iter()
            .any(|event| matches!(event, ApplyEvent::Progress { percent: 100, .. })));
        assert!(events.iter().any(
            |event| matches!(event, ApplyEvent::Log(line) if line.contains("upgrading linux"))
        ));
    }

    #[tokio::test]
    async fn streaming_apply_reports_failure_candidates() {
        let script = fake_pacman(
            "#!/bin/sh\n\
             echo \"error: failed to commit transaction (conflicting files)\"\n\
             echo \"/usr/lib/foo: /usr/lib/foo exists in both 'mpv' and 'mpv-old'\"\n\
             exit 1\n",
        );
        let (tx, mut rx) = mpsc::channel(64);
        let request = request(&[UpdateSource::System], &[]);
        let outcome = apply_streaming(
            &request,
            false,
            script.to_str().expect("script path"),
            "/bin/true",
            tx,
            CancelHandle::default(),
        )
        .await
        .expect("apply");
        let _ = drain(&mut rx).await;

        assert!(!outcome.success);
        assert!(!outcome.cancelled);
        assert_eq!(
            outcome.error.as_deref(),
            Some("error: failed to commit transaction (conflicting files)")
        );
        assert_eq!(
            outcome.candidates,
            vec!["mpv".to_string(), "mpv-old".to_string()]
        );
    }

    #[tokio::test]
    async fn cancellation_stops_pacman() {
        let script = fake_pacman(
            "#!/bin/sh\n\
             trap 'exit 130' INT TERM\n\
             echo '(1/2) upgrading linux'\n\
             sleep 5\n",
        );
        let (tx, mut rx) = mpsc::channel(64);
        let request = request(&[UpdateSource::System], &[]);
        let cancel = CancelHandle::default();
        let canceller = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            canceller.cancel();
        });

        let outcome = apply_streaming(
            &request,
            false,
            script.to_str().expect("script path"),
            "/bin/true",
            tx,
            cancel,
        )
        .await
        .expect("apply");
        let _ = drain(&mut rx).await;

        assert!(!outcome.success);
        assert!(outcome.cancelled);
        assert_eq!(outcome.error.as_deref(), Some("Update cancelled."));
    }

    #[tokio::test]
    async fn flatpak_system_apply_streams_logs() {
        let script = fake_flatpak(
            "#!/bin/sh\n\
             echo 'Looking for updates...'\n\
             echo 'Updating org.gimp.GIMP'\n\
             echo 'Updating org.freedesktop.Platform'\n\
             exit 0\n",
        );
        let (tx, mut rx) = mpsc::channel(64);
        let request = request(&[UpdateSource::FlatpakSystem], &[]);
        let outcome = apply_streaming(
            &request,
            false,
            "/bin/true",
            script.to_str().expect("script path"),
            tx,
            CancelHandle::default(),
        )
        .await
        .expect("apply");
        let events = drain(&mut rx).await;

        assert!(outcome.success);
        assert!(outcome.log_tail.contains("Updating org.gimp.GIMP"));
        assert!(events
            .iter()
            .any(|event| matches!(event, ApplyEvent::Progress { percent: 95, .. })));
        assert!(events
            .iter()
            .any(|event| matches!(event, ApplyEvent::Progress { percent: 100, .. })));
    }

    #[tokio::test]
    async fn mixed_apply_runs_system_then_flatpak() {
        let pacman = fake_pacman("#!/bin/sh\necho 'system line'\nexit 0\n");
        let flatpak = fake_flatpak("#!/bin/sh\necho 'flatpak line'\nexit 0\n");
        let (tx, mut rx) = mpsc::channel(64);
        let request = request(&[UpdateSource::System, UpdateSource::FlatpakSystem], &[]);
        let outcome = apply_streaming(
            &request,
            false,
            pacman.to_str().expect("pacman path"),
            flatpak.to_str().expect("flatpak path"),
            tx,
            CancelHandle::default(),
        )
        .await
        .expect("apply");
        let _ = drain(&mut rx).await;

        assert!(outcome.success);
        let system = outcome.log_tail.find("system line").expect("system log");
        let flatpak = outcome.log_tail.find("flatpak line").expect("flatpak log");
        assert!(system < flatpak);
    }

    #[tokio::test]
    async fn flatpak_user_is_rejected_by_the_helper() {
        let (tx, _rx) = mpsc::channel(4);
        let request = request(&[UpdateSource::FlatpakUser], &[]);
        let result = apply_streaming(
            &request,
            false,
            "/bin/true",
            "/bin/true",
            tx,
            CancelHandle::default(),
        )
        .await;
        assert!(result.is_err());
    }

    #[test]
    fn repositories_are_sorted_from_the_sync_directory() {
        let repositories = scan_sync_repositories();
        let mut sorted = repositories.clone();
        sorted.sort();
        assert_eq!(repositories, sorted);
    }

    #[test]
    #[ignore = "reads the host pacman databases"]
    fn reads_the_host_plan() {
        let plan = check().expect("host plan");
        assert!(!plan.checked_at.is_empty());
        eprintln!(
            "{} updates, {} bytes, reboot suggested: {}",
            plan.items.len(),
            plan.total_download_bytes,
            plan.reboot_suggested
        );
        for item in &plan.items {
            assert_eq!(item.source, UpdateSource::System);
            assert!(!item.name.is_empty());
            assert!(!item.new_version.is_empty());
            eprintln!(
                "  {}/{} {} -> {} ({})",
                item.repo, item.name, item.current_version, item.new_version, item.download_bytes
            );
        }

        if is_root() {
            let decision = prepare(&[]).expect("host prepare");
            assert!(decision.safe, "{}", decision.reason);
            eprintln!("full transaction: {}", decision.reason);

            if let Some(first) = plan.items.first() {
                let decision = prepare(std::slice::from_ref(&first.name)).expect("hold prepare");
                eprintln!(
                    "holding {}: {} ({})",
                    first.name, decision.safe, decision.reason
                );
            }

            let decision = prepare(&["definitely-not-a-package".to_string()]).expect("bad hold");
            assert!(!decision.safe);
        }
    }

    fn is_root() -> bool {
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|status| {
                status
                    .lines()
                    .find(|line| line.starts_with("Uid:"))
                    .and_then(|line| line.split_whitespace().nth(2).map(str::to_string))
            })
            .is_some_and(|uid| uid == "0")
    }

    fn request(sources: &[UpdateSource], holds: &[&str]) -> ApplyRequest {
        ApplyRequest {
            sources: sources.to_vec(),
            holds: holds.iter().map(|hold| hold.to_string()).collect(),
        }
    }

    async fn drain(rx: &mut mpsc::Receiver<ApplyEvent>) -> Vec<ApplyEvent> {
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        events
    }

    fn fake_pacman(body: &str) -> PathBuf {
        fake_program("pacman", body)
    }

    fn fake_flatpak(body: &str) -> PathBuf {
        fake_program("flatpak", body)
    }

    fn fake_program(name: &str, body: &str) -> PathBuf {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let directory =
            std::env::temp_dir().join(format!("skemer-{name}-{}-{id}", std::process::id()));
        fs::create_dir_all(&directory).expect("temp directory");
        let path = directory.join(name);
        let mut file = fs::File::create(&path).expect("create fake program");
        file.write_all(body.as_bytes()).expect("write fake program");
        file.sync_all().expect("flush fake program");
        drop(file);
        let mut permissions = fs::metadata(&path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("chmod");
        path
    }
}
