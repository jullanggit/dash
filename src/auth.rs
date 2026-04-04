#[cfg(feature = "server")]
use anyhow::{Context, anyhow};
use dioxus::fullstack::{
    FullstackContext,
    http::{HeaderValue, header},
};
use dioxus::prelude::*;
use std::{
    borrow::Cow,
    collections::HashMap,
    env,
    path::Path,
    sync::{LazyLock, Mutex},
};
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

#[cfg(feature = "server")]
fn current_session_id() -> Option<String> {
    let context = FullstackContext::current()?;
    let parts = context.parts_mut();
    let cookie_header = parts.headers.get(header::COOKIE)?.to_str().ok()?;

    parse_cookie(cookie_header, SESSION_COOKIE_NAME)
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
    use std::path::Path;

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
        .verify_password(password.as_bytes(), &parsed_hash)
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
pub async fn assert_authenticated() -> Result<()> {
    let err = anyhow!("unauthenticated").into();
    let Some(session_id) = current_session_id() else {
        return Err(err);
    };

    let now = UtcDateTime::now();
    let mut sessions = SESSIONS.lock().expect("session mutex poisoned");
    sessions.retain(|_, expires_at| *expires_at > now);

    if sessions.contains_key(&session_id) {
        Ok(())
    } else {
        Err(err)
    }
}

#[server]
pub async fn login(password: String) -> Result<()> {
    if !verify_password(&password).await? {
        return Err(anyhow!("unauthenticated").into());
    }

    let session_id = create_session();
    set_session_cookie(&session_id)?;

    Ok(())
}

#[macro_export]
macro_rules! assert_authenticated {
    () => {
        $crate::auth::assert_authenticated().await?;
    };
}
