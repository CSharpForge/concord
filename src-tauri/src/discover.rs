//! Discovery: list the servers, channels and DMs the signed-in user can see.
//! Read-only. The token is loaded from the keychain here and never returned.

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::auth::{AuthState, EVENT_AUTH_CHANGED};
use crate::discord::model::{self, ChannelGroup, DmInfo, GuildInfo};
use crate::discord::{ApiError, CurrentUser, DiscordClient};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GuildChannels {
    pub groups: Vec<ChannelGroup>,
    /// True if active threads could not be listed (the rest is still valid).
    pub threads_incomplete: bool,
}

pub async fn load_guilds(client: &DiscordClient, token: &str) -> Result<Vec<GuildInfo>, ApiError> {
    let mut guilds: Vec<GuildInfo> = client.guilds(token).await?.into_iter().map(model::guild_info).collect();
    guilds.sort_by_key(|g| g.name.to_lowercase());
    Ok(guilds)
}

pub async fn load_dms(client: &DiscordClient, token: &str) -> Result<Vec<DmInfo>, ApiError> {
    Ok(model::dm_list(client.private_channels(token).await?))
}

pub async fn load_guild_channels(
    client: &DiscordClient,
    token: &str,
    guild_id: &str,
) -> Result<GuildChannels, ApiError> {
    let channels = client.guild_channels(token, guild_id).await?;
    let (threads, threads_incomplete) = match client.active_threads(token, guild_id).await {
        Ok(t) => (t, false),
        Err(ApiError::Unauthorized) => return Err(ApiError::Unauthorized),
        Err(_) => (Vec::new(), true),
    };
    Ok(GuildChannels { groups: model::group_channels(channels, threads), threads_incomplete })
}

// ---------------------------------------------------------------- Tauri glue

fn token(state: &AuthState) -> Result<String, String> {
    state.store.load().map_err(|e| e.to_string())?.ok_or_else(|| "Not signed in".to_string())
}

/// Turn an API error into a UI message. A 401 means the token died: forget it
/// and tell the UI so it returns to the sign-in screen.
fn report(app: &AppHandle, state: &AuthState, e: ApiError) -> String {
    if e == ApiError::Unauthorized {
        let _ = state.store.delete();
        let _ = app.emit(EVENT_AUTH_CHANGED, Option::<CurrentUser>::None);
    }
    e.to_string()
}

#[tauri::command]
pub async fn list_guilds(app: AppHandle, state: State<'_, AuthState>) -> Result<Vec<GuildInfo>, String> {
    let t = token(&state)?;
    load_guilds(&state.client, &t).await.map_err(|e| report(&app, &state, e))
}

#[tauri::command]
pub async fn list_dms(app: AppHandle, state: State<'_, AuthState>) -> Result<Vec<DmInfo>, String> {
    let t = token(&state)?;
    load_dms(&state.client, &t).await.map_err(|e| report(&app, &state, e))
}

#[tauri::command]
pub async fn list_guild_channels(
    app: AppHandle,
    state: State<'_, AuthState>,
    guild_id: String,
) -> Result<GuildChannels, String> {
    if !model::is_snowflake(&guild_id) {
        return Err("Invalid server id".into());
    }
    let t = token(&state)?;
    load_guild_channels(&state.client, &t, &guild_id).await.map_err(|e| report(&app, &state, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn guilds_sorted_case_insensitively() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/users/@me/guilds"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"id": "1", "name": "zebra"}, {"id": "2", "name": "Apple"}, {"id": "3", "name": "mango"}
            ])))
            .mount(&server).await;
        let g = load_guilds(&DiscordClient::new(server.uri()), "t").await.unwrap();
        assert_eq!(g.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["Apple", "mango", "zebra"]);
    }

    #[tokio::test]
    async fn thread_failure_is_partial_not_fatal() {
        let server = MockServer::start().await;
        Mock::given(path("/guilds/7/channels"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"id": "5", "type": 0, "name": "general", "position": 0}
            ])))
            .mount(&server).await;
        Mock::given(path("/guilds/7/threads/active")).respond_with(ResponseTemplate::new(403)).mount(&server).await;
        let r = load_guild_channels(&DiscordClient::new(server.uri()), "t", "7").await.unwrap();
        assert!(r.threads_incomplete);
        assert_eq!(r.groups[0].channels[0].name, "general");
    }

    #[tokio::test]
    async fn channels_failure_is_fatal_and_401_propagates() {
        let server = MockServer::start().await;
        Mock::given(path("/guilds/7/channels")).respond_with(ResponseTemplate::new(401)).mount(&server).await;
        let err = load_guild_channels(&DiscordClient::new(server.uri()), "t", "7").await.unwrap_err();
        assert_eq!(err, ApiError::Unauthorized);
    }

    #[tokio::test]
    async fn dms_loaded_and_named() {
        let server = MockServer::start().await;
        Mock::given(path("/users/@me/channels"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"id": "9", "type": 1, "recipients": [{"id": "5", "username": "zed", "global_name": null}]}
            ])))
            .mount(&server).await;
        let d = load_dms(&DiscordClient::new(server.uri()), "t").await.unwrap();
        assert_eq!(d[0].name, "zed");
    }
}
