mod auth;
mod background;
mod discord;
mod discover;
mod jobs;

use std::sync::Arc;

pub fn run() {
    let mut builder = tauri::Builder::default();

    // Official plugins (desktop only). Single-instance must be registered first.
    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // A second launch (e.g. from the Start menu) brings the running copy to the front.
            background::show_main_window(app);
        }));
        builder = builder.plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![background::START_MINIMIZED_ARG]),
        ));
    }

    builder
        .plugin(tauri_plugin_sql::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .manage(auth::AuthState {
            store: Arc::new(auth::token_store::KeyringStore::new()),
            client: discord::DiscordClient::with_default_base(),
        })
        .on_window_event(|window, event| background::on_window_event(window, event))
        .setup(|app| {
            let handle = app.handle().clone();
            tauri::async_runtime::block_on(jobs::init(&handle))?;
            let args: Vec<String> = std::env::args().collect();
            background::setup(&handle, &args)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            auth::auth_status,
            auth::auth_start_login,
            auth::auth_logout,
            discover::list_guilds,
            discover::list_dms,
            discover::list_guild_channels,
            jobs::jobs_list,
            jobs::jobs_create,
            jobs::job_pause,
            jobs::job_resume,
            jobs::job_cancel,
            jobs::job_remove,
            jobs::job_candidates,
            jobs::job_candidates_exclude,
            jobs::job_candidates_exclude_matching,
            jobs::protect_list,
            jobs::protect_add,
            jobs::protect_remove,
            background::background_status,
            background::background_set_autostart,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
