//! Minimal Discord API client.
//!
//! NOTE: using a *user* token against Discord's API is not supported by Discord
//! and can get an account terminated. See README. We send honest headers
//! (`User-Agent: Concord/<version>`) and never attempt to impersonate the
//! official client.

use std::fmt;

pub mod model;

use serde::{Deserialize, Serialize};

pub const DEFAULT_API_BASE: &str = "https://discord.com/api/v9";

/// The subset of `GET /users/@me` that Concord needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrentUser {
    pub id: String,
    pub username: String,
    #[serde(default)]
    pub global_name: Option<String>,
    #[serde(default)]
    pub avatar: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// 401: the token is invalid or was revoked.
    Unauthorized,
    /// 429: wait `retry_after_ms` before retrying.
    RateLimited { retry_after_ms: u64 },
    /// Any other non-success status.
    Http(u16),
    /// Connection problems, bad body, etc. Never contains the token.
    Network(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Unauthorized => write!(f, "Discord rejected the login (token invalid or revoked)"),
            ApiError::RateLimited { retry_after_ms } => {
                write!(f, "Rate limited by Discord; retry in {retry_after_ms} ms")
            }
            ApiError::Http(code) => write!(f, "Discord returned HTTP {code}"),
            ApiError::Network(msg) => write!(f, "Network error: {msg}"),
        }
    }
}

impl std::error::Error for ApiError {}

/// What Discord told us about the rate-limit bucket of a response.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RateInfo {
    pub remaining: Option<u32>,
    pub reset_after_ms: Option<u64>,
    pub bucket: Option<String>,
}

impl RateInfo {
    fn from_headers(h: &reqwest::header::HeaderMap) -> Self {
        let get = |name: &str| h.get(name).and_then(|v| v.to_str().ok());
        RateInfo {
            remaining: get("x-ratelimit-remaining").and_then(|s| s.parse().ok()),
            reset_after_ms: get("x-ratelimit-reset-after")
                .and_then(|s| s.parse::<f64>().ok())
                .map(|secs| (secs * 1000.0).ceil() as u64),
            bucket: get("x-ratelimit-bucket").map(str::to_owned),
        }
    }
}

#[derive(Clone)]
pub struct DiscordClient {
    http: reqwest::Client,
    base: String,
}

impl DiscordClient {
    pub fn new(base: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("Concord/", env!("CARGO_PKG_VERSION")))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("reqwest client configuration is static and valid");
        Self { http, base: base.into().trim_end_matches('/').to_owned() }
    }

    pub fn with_default_base() -> Self {
        Self::new(DEFAULT_API_BASE)
    }

    /// Send a request with the raw user token (no `Bearer`/`Bot` prefix) and map
    /// every non-2xx status to an `ApiError`. Never puts the token in errors.
    async fn send(
        &self,
        method: reqwest::Method,
        token: &str,
        path: &str,
    ) -> Result<(reqwest::Response, RateInfo), ApiError> {
        let resp = self
            .http
            .request(method, format!("{}{}", self.base, path))
            .header(reqwest::header::AUTHORIZATION, token)
            .send()
            .await
            .map_err(|e| ApiError::Network(e.without_url().to_string()))?;
        let rate = RateInfo::from_headers(resp.headers());
        match resp.status().as_u16() {
            200..=299 => Ok((resp, rate)),
            401 => Err(ApiError::Unauthorized),
            429 => {
                let retry_after_ms = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<f64>().ok())
                    .map(|secs| (secs * 1000.0).ceil() as u64)
                    .unwrap_or(1000);
                Err(ApiError::RateLimited { retry_after_ms })
            }
            code => Err(ApiError::Http(code)),
        }
    }

    async fn get_json_with_rate<T: serde::de::DeserializeOwned>(
        &self,
        token: &str,
        path: &str,
    ) -> Result<(T, RateInfo), ApiError> {
        let (resp, rate) = self.send(reqwest::Method::GET, token, path).await?;
        let body = resp
            .json::<T>()
            .await
            .map_err(|e| ApiError::Network(format!("unexpected response body: {}", e.without_url())))?;
        Ok((body, rate))
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, token: &str, path: &str) -> Result<T, ApiError> {
        Ok(self.get_json_with_rate(token, path).await?.0)
    }

    /// `GET /users/@me` — also used to validate a token.
    pub async fn current_user(&self, token: &str) -> Result<CurrentUser, ApiError> {
        self.get_json(token, "/users/@me").await
    }

    /// `GET /users/@me/guilds` (documented; up to 200 guilds, no paging needed).
    pub async fn guilds(&self, token: &str) -> Result<Vec<model::RawGuild>, ApiError> {
        self.get_json(token, "/users/@me/guilds").await
    }

    /// `GET /users/@me/channels` — existing DMs. NOT documented by Discord.
    pub async fn private_channels(&self, token: &str) -> Result<Vec<model::RawChannel>, ApiError> {
        self.get_json(token, "/users/@me/channels").await
    }

    /// `GET /guilds/{id}/channels` (documented; excludes threads).
    pub async fn guild_channels(&self, token: &str, guild_id: &str) -> Result<Vec<model::RawChannel>, ApiError> {
        if !model::is_snowflake(guild_id) {
            return Err(ApiError::Http(400));
        }
        self.get_json(token, &format!("/guilds/{guild_id}/channels")).await
    }

    /// `GET /guilds/{id}/threads/active` (documented).
    pub async fn active_threads(&self, token: &str, guild_id: &str) -> Result<Vec<model::RawChannel>, ApiError> {
        if !model::is_snowflake(guild_id) {
            return Err(ApiError::Http(400));
        }
        let list: model::RawThreadList = self.get_json(token, &format!("/guilds/{guild_id}/threads/active")).await?;
        Ok(list.threads)
    }

    /// `GET /channels/{id}/messages?limit=&before=` (documented): newest first,
    /// 1-100 per page.
    pub async fn channel_messages(
        &self,
        token: &str,
        channel_id: &str,
        before: Option<&str>,
        limit: u8,
    ) -> Result<(Vec<model::RawMessage>, RateInfo), ApiError> {
        if !model::is_snowflake(channel_id) || before.is_some_and(|b| !model::is_snowflake(b)) {
            return Err(ApiError::Http(400));
        }
        let limit = limit.clamp(1, 100);
        let mut path = format!("/channels/{channel_id}/messages?limit={limit}");
        if let Some(b) = before {
            path.push_str(&format!("&before={b}"));
        }
        self.get_json_with_rate(token, &path).await
    }

    /// `DELETE /channels/{id}/messages/{id}` (documented; 204 on success).
    pub async fn delete_message(
        &self,
        token: &str,
        channel_id: &str,
        message_id: &str,
    ) -> Result<RateInfo, ApiError> {
        if !model::is_snowflake(channel_id) || !model::is_snowflake(message_id) {
            return Err(ApiError::Http(400));
        }
        let (_, rate) = self
            .send(reqwest::Method::DELETE, token, &format!("/channels/{channel_id}/messages/{message_id}"))
            .await?;
        Ok(rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn current_user_sends_raw_token_and_parses_user() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/users/@me"))
            .and(header("authorization", "tok.en.123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "42", "username": "alice", "global_name": "Alice", "avatar": null, "extra": 1
            })))
            .expect(1)
            .mount(&server)
            .await;

        let user = DiscordClient::new(server.uri()).current_user("tok.en.123").await.unwrap();
        assert_eq!(user.id, "42");
        assert_eq!(user.username, "alice");
        assert_eq!(user.global_name.as_deref(), Some("Alice"));
    }

    #[tokio::test]
    async fn current_user_maps_401_to_unauthorized() {
        let server = MockServer::start().await;
        Mock::given(path("/users/@me")).respond_with(ResponseTemplate::new(401)).mount(&server).await;
        let err = DiscordClient::new(server.uri()).current_user("bad").await.unwrap_err();
        assert_eq!(err, ApiError::Unauthorized);
    }

    #[tokio::test]
    async fn current_user_maps_429_with_retry_after() {
        let server = MockServer::start().await;
        Mock::given(path("/users/@me"))
            .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "1.5"))
            .mount(&server)
            .await;
        let err = DiscordClient::new(server.uri()).current_user("t").await.unwrap_err();
        assert_eq!(err, ApiError::RateLimited { retry_after_ms: 1500 });
    }

    #[tokio::test]
    async fn current_user_maps_other_status() {
        let server = MockServer::start().await;
        Mock::given(path("/users/@me")).respond_with(ResponseTemplate::new(503)).mount(&server).await;
        let err = DiscordClient::new(server.uri()).current_user("t").await.unwrap_err();
        assert_eq!(err, ApiError::Http(503));
    }

    #[tokio::test]
    async fn network_error_never_contains_token() {
        // Nothing listens on this port.
        let err = DiscordClient::new("http://127.0.0.1:9").current_user("SECRET-TOKEN").await.unwrap_err();
        assert!(matches!(err, ApiError::Network(_)));
        assert!(!err.to_string().contains("SECRET-TOKEN"));
    }

    #[tokio::test]
    async fn discovery_endpoints_hit_documented_paths() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/users/@me/guilds")).and(header("authorization", "t"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"id": "1", "name": "A", "icon": null, "owner": true, "permissions": "1"}
            ]))).expect(1).mount(&server).await;
        Mock::given(method("GET")).and(path("/users/@me/channels"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([]))).expect(1).mount(&server).await;
        Mock::given(method("GET")).and(path("/guilds/77/channels"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"id": "5", "type": 0, "name": "general", "position": 0}
            ]))).expect(1).mount(&server).await;
        Mock::given(method("GET")).and(path("/guilds/77/threads/active"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "threads": [{"id": "9", "type": 11, "name": "t", "parent_id": "5"}], "members": []
            }))).expect(1).mount(&server).await;

        let c = DiscordClient::new(server.uri());
        assert_eq!(c.guilds("t").await.unwrap()[0].name, "A");
        assert!(c.private_channels("t").await.unwrap().is_empty());
        assert_eq!(c.guild_channels("t", "77").await.unwrap()[0].name.as_deref(), Some("general"));
        assert_eq!(c.active_threads("t", "77").await.unwrap()[0].id, "9");
    }

    #[tokio::test]
    async fn forbidden_guild_maps_to_http_403() {
        let server = MockServer::start().await;
        Mock::given(path("/guilds/77/channels")).respond_with(ResponseTemplate::new(403)).mount(&server).await;
        let err = DiscordClient::new(server.uri()).guild_channels("t", "77").await.unwrap_err();
        assert_eq!(err, ApiError::Http(403));
    }
}
