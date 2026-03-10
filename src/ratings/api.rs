use dioxus::fullstack::get_server_url;
use futures::StreamExt;
use rspotify::{
    AuthCodeSpotify, Config, Credentials, OAuth, model::SimplifiedPlaylist, prelude::OAuthClient,
    scopes,
};
use std::{
    collections::HashMap,
    convert::identity,
    sync::{
        OnceLock,
        atomic::{AtomicI64, AtomicU64, Ordering},
    },
};
use time::{Duration, UtcDateTime};
use tokio::sync::RwLock;

static SPOTIFY: OnceLock<AuthCodeSpotify> = OnceLock::new();

pub async fn spotify() -> &'static AuthCodeSpotify {
    match SPOTIFY.get() {
        Some(spotify) => spotify,
        None => {
            let spotify = AuthCodeSpotify::with_config(
                Credentials::from_env().expect("Failed to get credentials"),
                OAuth {
                    redirect_uri: "http://127.0.0.1:8888".into(), // TODO: get the actual url
                    scopes: scopes!(
                        "user-read-playback-state",
                        "playlist-read-private",
                        "playlist-read-collaborative"
                    ),
                    ..Default::default()
                },
                Config {
                    token_cached: true,
                    token_refreshing: true,
                    ..Default::default()
                },
            );
            let url = spotify
                .get_authorize_url(false)
                .expect("Should be able to get authorization url");

            spotify
                .prompt_for_token(&url)
                .await
                .expect("Should be able to authenticate");

            SPOTIFY.get_or_init(|| spotify)
        }
    }
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DiscreteRating(pub u8);
impl DiscreteRating {
    const FACTOR: f32 = 4.0;
    pub fn from_float(float: f32) -> Self {
        Self((float * Self::FACTOR) as u8)
    }
    pub fn to_float(self) -> f32 {
        (self.0 as f32) / Self::FACTOR
    }
}

static RATING_PLAYLISTS: RwLock<Option<HashMap<DiscreteRating, SimplifiedPlaylist>>> =
    RwLock::const_new(None);

static RATING_PLAYLISTS_LAST_FETCH: AtomicI64 = AtomicI64::new(0);

pub async fn rating_playlists() -> HashMap<DiscreteRating, SimplifiedPlaylist> {
    let now = UtcDateTime::now();

    let last_fetched = RATING_PLAYLISTS_LAST_FETCH.load(Ordering::Relaxed);
    if (now - Duration::minutes(1)).unix_timestamp() > last_fetched
        && RATING_PLAYLISTS_LAST_FETCH
            .compare_exchange(
                last_fetched,
                now.unix_timestamp(),
                Ordering::Relaxed,
                Ordering::Relaxed,
            )
            .is_ok()
        || RATING_PLAYLISTS.read().await.is_none()
    {
        let spotify = spotify().await;
        let mut playlists = HashMap::new();

        let mut stream = spotify.current_user_playlists();
        while let Some(playlist) = stream.next().await {
            if let Ok(playlist) = playlist
                && let Ok(rating) = playlist.name.parse::<f32>()
                && (0.0..=5.0).contains(&rating)
            {
                let rating = DiscreteRating::from_float(rating);
                match playlists.get(&rating) {
                    Some(rating) => panic!("Rating folder already present"),
                    None => playlists.insert(rating, playlist.clone()),
                };
            }
        }

        let clone = playlists.clone();
        tokio::spawn(async move { *RATING_PLAYLISTS.write().await = Some(clone) }); // write to rating playlists in the background

        playlists
    } else {
        RATING_PLAYLISTS
            .read()
            .await
            .as_ref()
            .expect("We check hashmap being present above")
            .clone()
    }
}
