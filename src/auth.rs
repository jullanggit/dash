#[cfg(feature = "login")]
mod gated {
    #[cfg(feature = "server")]
    use anyhow::{Context, anyhow};
    #[cfg(feature = "server")]
    use dioxus::fullstack::{
        FullstackContext,
        http::{HeaderValue, header},
    };
    use dioxus::prelude::*;
    use std::{borrow::Cow, env, path::Path};
    #[cfg(feature = "server")]
    use std::{
        collections::HashMap,
        sync::{LazyLock, Mutex},
    };
    #[cfg(feature = "server")]
    use time::{Duration, UtcDateTime};

    pub const SESSION_COOKIE_NAME: &str = "dashboard_session";

    #[cfg(feature = "server")]
    const SESSION_MAX_AGE: Duration = Duration::days(30);

    #[cfg(feature = "server")]
    static SESSIONS: LazyLock<Mutex<HashMap<String, UtcDateTime>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    #[cfg(feature = "server")]
    fn parse_cookie(cookie_header: &str, name: &str) -> Option<String> {
        cookie_header.split(';').find_map(|cookie| {
            let (key, value) = cookie.trim().split_once('=')?;
            (key == name).then(|| value.to_string())
        })
    }

    #[cfg(feature = "server")]
    fn session_expiry() -> UtcDateTime {
        UtcDateTime::now() + SESSION_MAX_AGE
    }

    enum SessionResult {
        Id(String),
        NoId,
        NoContext,
    }

    #[cfg(feature = "server")]
    fn current_session_id() -> SessionResult {
        let context = match FullstackContext::current() {
            Some(context) => context,
            None => return SessionResult::NoContext,
        };
        trace!("Current context: {context:?}");
        let parts = context.parts_mut();

        parts
            .headers
            .get_all(header::COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .find_map(|cookie_header| parse_cookie(cookie_header, SESSION_COOKIE_NAME))
            .map(SessionResult::Id)
            .unwrap_or(SessionResult::NoId)
    }

    #[cfg(feature = "server")]
    fn set_session_cookie(session_id: &str) -> Result<()> {
        let cookie = format!(
            "{SESSION_COOKIE_NAME}={session_id}; HttpOnly; Path=/; SameSite=Lax; Max-Age={}",
            SESSION_MAX_AGE.whole_seconds()
        );
        let header = HeaderValue::from_str(&cookie).context("failed to encode session cookie")?;
        let Some(context) = FullstackContext::current() else {
            return Err(anyhow!("failed to access server context").into());
        };

        context.add_response_header(header::SET_COOKIE, header);
        Ok(())
    }

    #[cfg(feature = "server")]
    fn create_session() -> String {
        use rand::distr::{Alphanumeric, SampleString};

        let session_id = Alphanumeric.sample_string(&mut rand::rng(), 64);
        let mut sessions = SESSIONS.lock().expect("session mutex poisoned");
        sessions.insert(session_id.clone(), session_expiry());
        session_id
    }

    #[cfg(feature = "server")]
    async fn verify_password(password: &str) -> Result<bool> {
        use crate::config::config_server;
        use argon2::{
            Argon2,
            password_hash::{PasswordHash, PasswordVerifier},
        };

        let config = config_server().await;
        let password_hash = tokio::fs::read_to_string(&expand_tilde(&config.password_file))
            .await
            .context("failed to read password file")?;
        let parsed_hash = PasswordHash::new(password_hash.trim())
            .map_err(|error| anyhow!("failed to parse password hash: {error}"))?;

        Ok(Argon2::default()
            .verify_password(password.trim().as_bytes(), &parsed_hash)
            .is_ok())
    }

    fn expand_tilde<'a>(path: &'a Path) -> Cow<'a, Path> {
        if path.starts_with("~") {
            let home = env::home_dir().expect("failed to get home directory");
            Cow::Owned(home.join(path.strip_prefix("~").unwrap()))
        } else {
            Cow::Borrowed(path)
        }
    }

    #[cfg(feature = "server")]
    fn is_authenticated_server() -> bool {
        let session_id = match current_session_id() {
            SessionResult::Id(session_id) => session_id,
            SessionResult::NoContext => return true, // server-to-server
            SessionResult::NoId => return false,
        };

        let now = UtcDateTime::now();
        let mut sessions = SESSIONS.lock().expect("session mutex poisoned");
        sessions.retain(|_, expires_at| *expires_at > now);
        trace!("Sessions: {sessions:?}");

        sessions.contains_key(&session_id)
    }

    #[server]
    pub async fn assert_authenticated() -> ServerFnResult<()> {
        if is_authenticated_server() {
            Ok(())
        } else {
            Err(ServerFnError::ServerError {
                message: "unauthenticated".to_string(),
                code: 401,
                details: None,
            })
        }
    }

    #[server]
    pub async fn login(password: String) -> Result<()> {
        if !verify_password(&password).await? {
            return Err(anyhow!("Wrong password").into());
        }

        let session_id = create_session();
        set_session_cookie(&session_id)?;

        Ok(())
    }
}

#[cfg(feature = "login")]
pub use gated::*;

#[macro_export]
macro_rules! assert_authenticated {
    () => {
        #[cfg(feature = "login")]
        {
            $crate::auth::assert_authenticated().await?;
        }
    };
}
