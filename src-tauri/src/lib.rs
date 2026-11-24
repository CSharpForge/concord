mod auth;
mod discord;
mod discover;
mod jobs;

use std::sync::Arc;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_sql::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .manage(auth::AuthState {
            store: Arc::new(auth::token_store::KeyringStore::new()),
            client: discord::DiscordClient::with_default_base(),
        })
        .setup(|app| {
            let handle = app.handle().clone();
            tauri::async_runtime::block_on(jobs::init(&handle))?;
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
