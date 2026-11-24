mod autostart;
mod checker;
mod cli;
mod commands;
mod flatpak;
mod helper;
mod planner;
mod state;
mod tray;

use skemer_core::Settings;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    if cli::try_run() {
        return;
    }

    let window_mode = std::env::args().any(|argument| argument == "--window");
    let settings = Settings::load().unwrap_or_default();

    let mut builder = tauri::Builder::default()
        .manage(state::AppState::new(settings))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::check_updates,
            commands::prepare_holds,
            commands::apply_updates,
            commands::cancel_updates,
            commands::search_packages,
            commands::package_info,
            commands::preview_packages,
            commands::apply_packages,
            commands::repository_status,
            commands::set_repository,
            commands::set_pacman_option,
            commands::load_settings,
            commands::save_settings,
        ]);

    if !window_mode {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if !argv.iter().any(|argument| argument == "--tray") {
                tray::open_window(app);
            }
        }));
    }

    let result = builder
        .setup(move |app| {
            if window_mode {
                tray::show_window(app.handle());
                tray::spawn_focus_handler(app.handle().clone());
            } else {
                tray::setup(app)?;
                checker::spawn(app.handle().clone());
                checker::spawn_child_reaper(app.handle().clone());
                checker::spawn_settings_reload(app.handle().clone());
                if !std::env::args().any(|argument| argument == "--tray") {
                    tray::open_window(app.handle());
                }
            }
            Ok(())
        })
        .build(tauri::generate_context!());

    match result {
        // The parent has no window and the window child exits on its own when
        // its window closes. Let Tauri handle session end and explicit exits.
        Ok(app) => app.run(|_app, _event| {}),
        Err(error) => {
            eprintln!("skemer: {error}");
            std::process::exit(1);
        }
    }
}
