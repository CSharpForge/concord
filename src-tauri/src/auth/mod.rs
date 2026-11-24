//! Authentication: embedded Discord login, keychain storage, session checks.

pub mod capture;
pub mod token_store;

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

use crate::discord::{ApiError, CurrentUser, DiscordClient};
use token_store::{StoreError, TokenStore};

const LOGIN_WINDOW: &str = "discord-login";
const LOGIN_URL: &str = "https://discord.com/login";
pub const EVENT_AUTH_CHANGED: &str = "auth-changed";
pub const EVENT_LOGIN_FAILED: &str = "auth-login-failed";

#[derive(Debug)]
pub enum AuthError {
    Api(ApiError),
    Store(StoreError),
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AuthError::Api(e) => e.fmt(f),
            AuthError::Store(e) => e.fmt(f),
        }
    }
}

impl From<ApiError> for AuthError {
    fn from(e: ApiError) -> Self {
        AuthError::Api(e)
    }
}
impl From<StoreError> for AuthError {
    fn from(e: StoreError) -> Self {
        AuthError::Store(e)
    }
}

/// Validate `token` with Discord and, only if valid, persist it.
pub async fn complete_login(
    store: &dyn TokenStore,
    client: &DiscordClient,
    token: &str,
) -> Result<CurrentUser, AuthError> {
    let user = client.current_user(token).await?;
    store.save(token)?;
    Ok(user)
}

/// The logged-in user, if a stored token is still valid.
/// A rejected (401) token is deleted; transient errors leave it in place.
pub async fn current_session(
    store: &dyn TokenStore,
    client: &DiscordClient,
) -> Result<Option<CurrentUser>, AuthError> {
    let Some(token) = store.load()? else { return Ok(None) };
    match client.current_user(&token).await {
        Ok(user) => Ok(Some(user)),
        Err(ApiError::Unauthorized) => {
            store.delete()?;
            Ok(None)
        }
        Err(e) => Err(e.into()),
    }
}

pub fn logout(store: &dyn TokenStore) -> Result<(), AuthError> {
    store.delete()?;
    Ok(())
}

// ---------------------------------------------------------------- Tauri glue

pub struct AuthState {
    pub store: Arc<dyn TokenStore>,
    pub client: DiscordClient,
}

#[tauri::command]
pub async fn auth_status(state: State<'_, AuthState>) -> Result<Option<CurrentUser>, String> {
    current_session(state.store.as_ref(), &state.client).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn auth_logout(app: AppHandle, state: State<'_, AuthState>) -> Result<(), String> {
    logout(state.store.as_ref()).map_err(|e| e.to_string())?;
    let _ = app.emit(EVENT_AUTH_CHANGED, Option::<CurrentUser>::None);
    Ok(())
}

#[tauri::command]
pub async fn auth_start_login(app: AppHandle) -> Result<(), String> {
    if let Some(existing) = app.get_webview_window(LOGIN_WINDOW) {
        return existing.set_focus().map_err(|e| e.to_string());
    }

    // Set while a captured token is being validated, so a burst of API calls
    // can't start several validations. Cleared again if validation fails.
    let claimed = Arc::new(AtomicBool::new(false));
    let nav_app = app.clone();

    let url: tauri::Url = LOGIN_URL.parse().map_err(|e: <tauri::Url as std::str::FromStr>::Err| e.to_string())?;
    WebviewWindowBuilder::new(&app, LOGIN_WINDOW, WebviewUrl::External(url))
        .title("Sign in to Discord — Concord")
        // Wide enough that Discord's login page shows its "Log in with QR code"
        // panel (it is hidden in narrow windows).
        .inner_size(960.0, 720.0)
        .initialization_script(capture::INIT_SCRIPT)
        .on_navigation(move |url| {
            if url.scheme() == capture::CAPTURE_SCHEME {
                if let Some(token) = capture::token_from_url(url) {
                    if !claimed.swap(true, Ordering::SeqCst) {
                        let app = nav_app.clone();
                        let claimed = claimed.clone();
                        tauri::async_runtime::spawn(async move {
                            finish_login(&app, &token).await;
                            claimed.store(false, Ordering::SeqCst);
                        });
                    }
                }
                return false; // never actually navigate to the capture URL
            }
            capture::is_allowed_navigation(url)
        })
        .build()
        .map_err(|e| e.to_string())?;
    Ok(())
}

async fn finish_login(app: &AppHandle, token: &str) {
    let state = app.state::<AuthState>();
    match complete_login(state.store.as_ref(), &state.client, token).await {
        Ok(user) => {
            if let Some(win) = app.get_webview_window(LOGIN_WINDOW) {
                // Don't leave a Discord session behind in the embedded webview.
                let _ = win.clear_all_browsing_data();
                let _ = win.close();
            }
            let _ = app.emit(EVENT_AUTH_CHANGED, Some(user));
        }
        Err(e) => {
            let _ = app.emit(EVENT_LOGIN_FAILED, e.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use token_store::MemoryStore;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn user_json() -> serde_json::Value {
        serde_json::json!({"id": "1", "username": "bob"})
    }

    #[tokio::test]
    async fn login_with_valid_token_saves_it() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/users/@me"))
            .and(header("authorization", "good"))
            .respond_with(ResponseTemplate::new(200).set_body_json(user_json()))
            .mount(&server)
            .await;
        let store = MemoryStore::default();
        let user = complete_login(&store, &DiscordClient::new(server.uri()), "good").await.unwrap();
        assert_eq!(user.username, "bob");
        assert_eq!(store.load().unwrap().as_deref(), Some("good"));
    }

    #[tokio::test]
    async fn login_with_invalid_token_saves_nothing() {
        let server = MockServer::start().await;
        Mock::given(path("/users/@me")).respond_with(ResponseTemplate::new(401)).mount(&server).await;
        let store = MemoryStore::default();
        let err = complete_login(&store, &DiscordClient::new(server.uri()), "bad").await.unwrap_err();
        assert!(matches!(err, AuthError::Api(ApiError::Unauthorized)));
        assert_eq!(store.load().unwrap(), None);
    }

    #[tokio::test]
    async fn session_none_without_token() {
        let server = MockServer::start().await; // no mocks: any request would 404
        let store = MemoryStore::default();
        assert_eq!(current_session(&store, &DiscordClient::new(server.uri())).await.unwrap(), None);
    }

    #[tokio::test]
    async fn session_valid_returns_user() {
        let server = MockServer::start().await;
        Mock::given(path("/users/@me"))
            .respond_with(ResponseTemplate::new(200).set_body_json(user_json()))
            .mount(&server)
            .await;
        let store = MemoryStore::default();
        store.save("t").unwrap();
        let u = current_session(&store, &DiscordClient::new(server.uri())).await.unwrap();
        assert_eq!(u.unwrap().id, "1");
    }

    #[tokio::test]
    async fn session_revoked_token_is_deleted() {
        let server = MockServer::start().await;
        Mock::given(path("/users/@me")).respond_with(ResponseTemplate::new(401)).mount(&server).await;
        let store = MemoryStore::default();
        store.save("revoked").unwrap();
        assert_eq!(current_session(&store, &DiscordClient::new(server.uri())).await.unwrap(), None);
        assert_eq!(store.load().unwrap(), None);
    }

    #[tokio::test]
    async fn session_transient_error_keeps_token() {
        let server = MockServer::start().await;
        Mock::given(path("/users/@me")).respond_with(ResponseTemplate::new(503)).mount(&server).await;
        let store = MemoryStore::default();
        store.save("keep").unwrap();
        let err = current_session(&store, &DiscordClient::new(server.uri())).await.unwrap_err();
        assert!(matches!(err, AuthError::Api(ApiError::Http(503))));
        assert_eq!(store.load().unwrap().as_deref(), Some("keep"));
    }

    #[test]
    fn logout_clears_token() {
        let store = MemoryStore::default();
        store.save("x").unwrap();
        logout(&store).unwrap();
        assert_eq!(store.load().unwrap(), None);
    }
}
