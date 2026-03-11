use dioxus::fullstack::get_server_url;
use futures::{FutureExt, StreamExt};
use rspotify::{
    AuthCodeSpotify, Config, Credentials, OAuth,
    model::{FullTrack, PlayableItem, PlaylistItem, SimplifiedPlaylist},
    prelude::{BaseClient, OAuthClient},
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

macro_rules! refreshing {
    ($fn_name:ident, $return:ty, $body:block, $const:ident) => {
        static $const: RwLock<Option<$return>> = RwLock::const_new(None);
        static ${ concat($const, _LAST_FETCH) }: AtomicI64 = AtomicI64::new(0);

        pub async fn $fn_name() -> $return {
            let now = UtcDateTime::now();

            let last_fetched = ${ concat($const, _LAST_FETCH) }.load(Ordering::Relaxed);
            if (now - Duration::minutes(1)).unix_timestamp() > last_fetched
                && ${ concat($const, _LAST_FETCH) }
                    .compare_exchange(
                        last_fetched,
                        now.unix_timestamp(),
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    )
                    .is_ok()
                || $const.read().await.is_none()
            {
                let new_value = $body;

                let clone = new_value.clone();
                tokio::spawn(async move { *$const.write().await = Some(clone) }); // update in the background

                new_value
            } else {
                $const
                    .read()
                    .await
                    .as_ref()
                    .expect("We check hashmap being present above")
                    .clone()
            }
        }
    };
}

refreshing!(rating_playlists, HashMap<DiscreteRating, SimplifiedPlaylist>, {
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
    playlists
}, RATING_PLAYLISTS);

struct Analyzation {
    canonical_rating: f32,
    rating_history: Vec<(UtcDateTime, f32)>,
}

refreshing!(ratings, HashMap<FullTrack, Analyzation>, {
    let spotify = spotify().await;
    let playlists = rating_playlists().await;
    let mut ratings = HashMap::new();

    for (rating, playlist) in playlists {
        spotify
            .playlist_items(playlist.id, None, None)
            .for_each(|result| async move {
                match result {
                   Ok(PlaylistItem {
                       added_at: Some(added_at),
                       track: Some(PlayableItem::Track(track)),
                       ..
                   }) =>  {},
                   _ => {},
                }
            })
            .await;
    }

    ratings
}, RATINGS);
