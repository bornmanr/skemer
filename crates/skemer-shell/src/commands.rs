use std::str::FromStr;

use skemer_core::models::{
    ApplyOutcome, ApplyRequest, HoldDecision, PackageAction, PackageDetails, PackageHit,
    PackageOutcome, PackagePreview, PackageQuery, PackageRequest, RepositoryStatus, UpdatePlan,
    UpdateSource,
};
use skemer_core::Settings;
use tauri::Manager;

use crate::state::AppState;
use crate::{autostart, flatpak, helper, planner, tray};

#[tauri::command]
pub async fn check_updates(app: tauri::AppHandle) -> Result<UpdatePlan, String> {
    let plan = planner::build().await.map_err(|error| error.to_string())?;
    tray::update_tooltip(&app, &plan);
    Ok(plan)
}

#[tauri::command]
pub async fn prepare_holds(holds: Vec<String>) -> Result<HoldDecision, String> {
    helper::prepare(holds)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn apply_updates(
    app: tauri::AppHandle,
    sources: Vec<String>,
    holds: Vec<String>,
) -> Result<ApplyOutcome, String> {
    let sources = sources
        .iter()
        .map(|source| UpdateSource::from_str(source))
        .collect::<Result<Vec<_>, _>>()?;

    let result = apply_sources(&app, sources, holds).await;
    // Ask the tray parent to refresh its tooltip and notification state.
    crate::state::signal_parent(libc::SIGUSR1);
    result
}

async fn apply_sources(
    app: &tauri::AppHandle,
    sources: Vec<UpdateSource>,
    holds: Vec<String>,
) -> Result<ApplyOutcome, String> {
    let wants_user = sources.contains(&UpdateSource::FlatpakUser);
    let helper_sources: Vec<UpdateSource> = sources
        .iter()
        .copied()
        .filter(|source| *source != UpdateSource::FlatpakUser)
        .collect();

    let mut outcome = if helper_sources.is_empty() {
        empty_outcome()
    } else {
        helper::apply(
            app.clone(),
            ApplyRequest {
                sources: helper_sources,
                holds,
            },
        )
        .await
        .map_err(|error| error.to_string())?
    };

    if outcome.success && wants_user {
        match flatpak::apply_user(app).await {
            Ok(user) => {
                outcome.log_tail = flatpak::combine_logs(outcome.log_tail, user.log_tail);
                if !user.success {
                    outcome.success = false;
                    outcome.cancelled = user.cancelled;
                    outcome.error = user.error;
                    outcome.candidates = user.candidates;
                }
            }
            Err(error) => outcome = failure_outcome(error.to_string()),
        }
    }

    Ok(outcome)
}

#[tauri::command]
pub async fn cancel_updates() -> Result<(), String> {
    flatpak::cancel_user();
    // The helper may not have a running job; cancel is still a no-op then.
    let _ = helper::cancel().await;
    Ok(())
}

#[tauri::command]
pub async fn search_packages(
    query: String,
    repo: Option<String>,
    installed_only: bool,
) -> Result<Vec<PackageHit>, String> {
    helper::search_packages(PackageQuery {
        query,
        repo,
        installed_only,
        limit: 50,
    })
    .await
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn package_info(name: String) -> Result<PackageDetails, String> {
    helper::package_info(name)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn preview_packages(
    action: String,
    names: Vec<String>,
) -> Result<PackagePreview, String> {
    let action = PackageAction::from_str(&action)?;
    helper::preview_packages(PackageRequest { action, names })
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn apply_packages(
    app: tauri::AppHandle,
    action: String,
    names: Vec<String>,
) -> Result<PackageOutcome, String> {
    let action = PackageAction::from_str(&action)?;
    let result = helper::apply_packages(app, PackageRequest { action, names })
        .await
        .map_err(|error| error.to_string());
    crate::state::signal_parent(libc::SIGUSR1);
    result
}

#[tauri::command]
pub async fn repository_status() -> Result<RepositoryStatus, String> {
    helper::repository_status()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn set_repository(name: String, enabled: bool) -> Result<RepositoryStatus, String> {
    helper::set_repository(name, enabled)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn set_pacman_option(name: String, value: String) -> Result<RepositoryStatus, String> {
    helper::set_pacman_option(name, value)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn load_settings() -> Result<Settings, String> {
    Settings::load().map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn save_settings(app: tauri::AppHandle, settings: Settings) -> Result<(), String> {
    settings.save().map_err(|error| error.to_string())?;
    autostart::apply(settings.autostart).map_err(|error| error.to_string())?;

    let state = app.state::<AppState>();
    state.set_settings(settings);
    state.settings_changed.notify_waiters();
    crate::state::signal_parent(libc::SIGHUP);
    Ok(())
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

fn failure_outcome(error: String) -> ApplyOutcome {
    ApplyOutcome {
        success: false,
        cancelled: false,
        held_packages: Vec::new(),
        reboot_suggested: false,
        log_tail: String::new(),
        error: Some(error),
        candidates: Vec::new(),
    }
}
