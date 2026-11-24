use serde::{Deserialize, Serialize};

/// One update source. The v1 UI exposes system, Flatpak system, and Flatpak
/// user. AUR is intentionally not part of the first release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateSource {
    System,
    FlatpakSystem,
    FlatpakUser,
}

impl UpdateSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::FlatpakSystem => "Flatpak system",
            Self::FlatpakUser => "Flatpak user",
        }
    }
}

impl std::str::FromStr for UpdateSource {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "system" => Ok(Self::System),
            "flatpak-system" => Ok(Self::FlatpakSystem),
            "flatpak-user" => Ok(Self::FlatpakUser),
            other => Err(format!("unknown update source: {other}")),
        }
    }
}

/// One pending package update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateItem {
    pub id: String,
    pub source: UpdateSource,
    pub name: String,
    pub repo: String,
    pub current_version: String,
    pub new_version: String,
    pub download_bytes: u64,
    pub installed_bytes: u64,
    pub summary: String,
    pub reboot_suggested: bool,
    pub warning: Option<String>,
}

/// The complete update plan shown to the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UpdatePlan {
    pub items: Vec<UpdateItem>,
    pub total_download_bytes: u64,
    pub reboot_suggested: bool,
    pub checked_at: String,
    pub warnings: Vec<String>,
    /// Packages held for the current transaction. These are not persisted.
    pub held_packages: Vec<String>,
}

impl UpdatePlan {
    pub fn from_items(items: Vec<UpdateItem>, checked_at: impl Into<String>) -> Self {
        let total_download_bytes = items.iter().map(|item| item.download_bytes).sum();
        let reboot_suggested = items.iter().any(|item| item.reboot_suggested);
        Self {
            items,
            total_download_bytes,
            reboot_suggested,
            checked_at: checked_at.into(),
            warnings: Vec::new(),
            held_packages: Vec::new(),
        }
    }

    pub fn source_count(&self, source: UpdateSource) -> usize {
        self.items
            .iter()
            .filter(|item| item.source == source)
            .count()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn sources(&self) -> Vec<UpdateSource> {
        let mut sources: Vec<UpdateSource> = self.items.iter().map(|item| item.source).collect();
        sources.sort();
        sources.dedup();
        sources
    }
}

/// Request passed to the privileged helper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyRequest {
    pub sources: Vec<UpdateSource>,
    /// Package names to hold for this transaction only. The helper validates
    /// these with libalpm before applying.
    #[serde(default)]
    pub holds: Vec<String>,
}

/// Result of an apply operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyOutcome {
    pub success: bool,
    #[serde(default)]
    pub cancelled: bool,
    pub held_packages: Vec<String>,
    pub reboot_suggested: bool,
    pub log_tail: String,
    /// Human-readable failure reason when `success` is false.
    #[serde(default)]
    pub error: Option<String>,
    /// Packages that look like the cause of a failure. These are hints for the
    /// UI; the helper validates any proposed hold with libalpm.
    #[serde(default)]
    pub candidates: Vec<String>,
}

/// Result of validating a proposed hold with libalpm.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoldDecision {
    pub safe: bool,
    pub reason: String,
    pub candidates: Vec<String>,
}

impl HoldDecision {
    pub fn safe(reason: impl Into<String>) -> Self {
        Self {
            safe: true,
            reason: reason.into(),
            candidates: Vec::new(),
        }
    }

    pub fn rejected(reason: impl Into<String>, candidates: Vec<String>) -> Self {
        Self {
            safe: false,
            reason: reason.into(),
            candidates,
        }
    }
}

/// Install or remove. Package management never downgrades and never touches
/// the AUR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackageAction {
    Install,
    Remove,
}

impl PackageAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Install => "Install",
            Self::Remove => "Remove",
        }
    }
}

impl std::str::FromStr for PackageAction {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "install" => Ok(Self::Install),
            "remove" => Ok(Self::Remove),
            other => Err(format!("unknown package action: {other}")),
        }
    }
}

/// Search request from the UI. The helper never accepts flags or paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageQuery {
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub installed_only: bool,
    #[serde(default = "default_search_limit")]
    pub limit: usize,
}

fn default_search_limit() -> usize {
    50
}

impl Default for PackageQuery {
    fn default() -> Self {
        Self {
            query: String::new(),
            repo: None,
            installed_only: false,
            limit: default_search_limit(),
        }
    }
}

/// One search result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageHit {
    pub name: String,
    pub repo: String,
    pub version: String,
    #[serde(default)]
    pub installed_version: Option<String>,
    #[serde(default)]
    pub download_bytes: u64,
    #[serde(default)]
    pub installed_bytes: u64,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub groups: Vec<String>,
}

/// Full metadata for one package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageDetails {
    pub name: String,
    #[serde(default)]
    pub repo: Option<String>,
    pub version: String,
    #[serde(default)]
    pub installed_version: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub packager: String,
    #[serde(default)]
    pub build_date: i64,
    #[serde(default)]
    pub download_bytes: u64,
    #[serde(default)]
    pub installed_bytes: u64,
    #[serde(default)]
    pub licenses: Vec<String>,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub depends: Vec<String>,
    #[serde(default)]
    pub optdepends: Vec<String>,
    #[serde(default)]
    pub conflicts: Vec<String>,
    #[serde(default)]
    pub replaces: Vec<String>,
    #[serde(default)]
    pub provides: Vec<String>,
    /// Installed packages that depend on this one, read from the local database.
    #[serde(default)]
    pub required_by: Vec<String>,
}

/// One package in a transaction preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackagePreviewItem {
    pub name: String,
    #[serde(default)]
    pub repo: Option<String>,
    pub version: String,
    #[serde(default)]
    pub current_version: Option<String>,
    #[serde(default)]
    pub download_bytes: u64,
    #[serde(default)]
    pub installed_bytes: u64,
}

/// Read-only resolution of a proposed install or removal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackagePreview {
    pub action: PackageAction,
    #[serde(default)]
    pub adds: Vec<PackagePreviewItem>,
    #[serde(default)]
    pub removes: Vec<PackagePreviewItem>,
    #[serde(default)]
    pub total_download_bytes: u64,
    #[serde(default)]
    pub total_installed_bytes: u64,
    /// Pending system updates that the install transaction would also apply.
    #[serde(default)]
    pub pending_system_updates: usize,
    #[serde(default)]
    pub reboot_suggested: bool,
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// The requested install or removal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageRequest {
    pub action: PackageAction,
    pub names: Vec<String>,
}

/// Result of an install or removal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageOutcome {
    pub success: bool,
    #[serde(default)]
    pub cancelled: bool,
    pub action: PackageAction,
    #[serde(default)]
    pub names: Vec<String>,
    #[serde(default)]
    pub reboot_suggested: bool,
    #[serde(default)]
    pub log_tail: String,
    #[serde(default)]
    pub error: Option<String>,
}

/// One repository as the helper sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryInfo {
    pub name: String,
    pub enabled: bool,
    pub source_file: String,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub toggleable: bool,
    #[serde(default)]
    pub package_count: usize,
    #[serde(default)]
    pub synced_at: Option<String>,
}

/// Effective repository and pacman option state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryStatus {
    pub pacman_conf: String,
    pub repositories: Vec<RepositoryInfo>,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub parallel_downloads: u32,
    #[serde(default)]
    pub parallel_downloads_editable: bool,
    #[serde(default)]
    pub parallel_downloads_source: Option<String>,
}
