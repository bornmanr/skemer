//! Skemer core: models, settings, plan helpers, and source traits.
//!
//! This crate deliberately has no Tauri, GTK, or D-Bus dependencies. It is
//! shared by the shell, the privileged helper, and the CLI.

pub mod cancel;
pub mod models;
pub mod packages;
pub mod pacman_conf;
pub mod pacman_config;
pub mod paths;
pub mod plan;
pub mod settings;
pub mod sources;

pub use cancel::CancelHandle;
pub use models::{
    ApplyOutcome, ApplyRequest, HoldDecision, PackageAction, PackageDetails, PackageHit,
    PackageOutcome, PackagePreview, PackagePreviewItem, PackageQuery, PackageRequest,
    RepositoryInfo, RepositoryStatus, UpdateItem, UpdatePlan, UpdateSource,
};
pub use pacman_conf::PacmanConf;
pub use settings::Settings;
