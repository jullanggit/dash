#[cfg(feature = "server")]
use crate::ratings::caching::caching;
use crate::{
    caching,
    ratings::analyze::{Analyzation, DEFAULT_RATING, TrackAnalyzation, analyze},
};
use dioxus::prelude::*;
use dioxus_sdk_time::use_interval;
#[cfg(feature = "server")]
use rspotify::{
    AuthCodeSpotify, ClientError, ClientResult, Config, Credentials, OAuth,
    prelude::{BaseClient, OAuthClient},
    scopes,
};
#[cfg(feature = "server")]
use rspotify_model::Page;
use rspotify_model::{
    CurrentPlaybackContext, PlayableItem, PlaylistItem, SimplifiedPlaylist, TrackId,
};
use serde::Serialize;
#[cfg(feature = "server")]
use serde::de::DeserializeOwned;
use std::sync::OnceLock;
use time::{Duration, UtcDateTime};
#[cfg(feature = "server")]
use tokio::{
    sync::{Mutex, RwLock},
    time::sleep,
};

#[cfg(feature = "server")]
static SPOTIFY: OnceLock<AuthCodeSpotify> = OnceLock::new();

// TODO: read credentials from config file (possibly with indirection for secrets) instead form .env
#[cfg(feature = "server")]
pub async fn spotify() -> &'static AuthCodeSpotify {
    println!("Getting spotify");

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

caching!(
    rating_playlists,
    Vec<(f32, SimplifiedPlaylist)>,
    async |_previous| {
        let spotify = spotify().await;
        let mut playlists = Vec::new();

        println!("Getting rating playlists");

        let page_results = paginate_retrying(move |offset| {
            let spotify = spotify.clone();
            async move {
                spotify
                    .current_user_playlists_manual(None, Some(offset))
                    .await
            }
        })
        .await;

        for page_result in page_results {
            match page_result {
                Ok(page) => {
                    for playlist in page.items {
                        if let Ok(rating) = playlist.name.parse::<f32>()
                            && (0.0..=5.0).contains(&rating)
                        {
                            if playlists.iter().any(|(s_rating, _)| *s_rating == rating) {
                                panic!("Rating folder already present")
                            } else {
                                playlists.push((rating, playlist.clone()))
                            };
                        }
                    }
                }
                Err(e) => eprintln!("Error getting playlist page: {e}"),
            }
        }

        playlists
    },
    RATING_PLAYLISTS,
    Duration::minutes(5)
);

#[cfg(feature = "server")]
async fn paginate_retrying<F, Fut, T>(f: F) -> Vec<ClientResult<Page<T>>>
where
    F: Fn(u32) -> Fut,
    Fut: Future<Output = ClientResult<Page<T>>>,
    T: DeserializeOwned,
{
    // rspotify's paginate() function streams these, but this is not really necessary here
    let mut out = Vec::new();

    let mut offset = 0;
    loop {
        let page = retrying(&f, offset).await;

        let mut end = false;
        if let Ok(ref page) = page {
            offset += page.items.len() as u32;
            end = page.next.is_none();
        }

        out.push(page);
        if end {
            break;
        }
    }
    out
}

/// Retries `f` if it got a too-many-requests error.
/// Suspends for the duration requested by the spotify API, which may be a long period of time.
#[cfg(feature = "server")]
async fn retrying<F, Args, Fut, T>(f: F, args: Args) -> ClientResult<T>
where
    F: Fn(Args) -> Fut,
    Args: Clone,
    Fut: Future<Output = ClientResult<T>>,
    T: DeserializeOwned,
{
    loop {
        let res = f(args.clone()).await;
        match res {
            Err(ClientError::Http(ref http)) => match http.as_ref() {
                rspotify_http::HttpError::StatusCode(response) => {
                    let code = response.status().as_u16();
                    if code == 429 {
                        let retry_after = response
                            .headers()
                            .iter()
                            .find(|(name, _)| name.as_str() == "retry-after")
                            .and_then(|(_, value)| value.to_str().ok())
                            .and_then(|str| str.parse().ok())
                            .unwrap_or(60);

                        // wait for retry-after, retry in the next loop, as offset didnt get incremented
                        println!("Retrying {} after {retry_after} seconds", response.url());
                        sleep(std::time::Duration::from_secs(retry_after)).await;
                        continue;
                    }
                }
                _ => {}
            },
            _ => {}
        }

        return res;
    }
}

caching!(
    ratings,
    Analyzation,
    async |_previous| {
        let spotify = spotify().await;
        let playlists = rating_playlists_server().await;
        let mut ratings = Vec::new();

        println!("Getting ratings");

        for (rating, playlist) in playlists {
            let page_results = paginate_retrying(move |offset| {
                let spotify = spotify.clone();
                let id = playlist.id.clone();
                async move {
                    spotify
                        .playlist_items_manual(id, None, None, None, Some(offset))
                        .await
                }
            })
            .await;

            for page_result in page_results {
                match page_result {
                    Ok(page) => {
                        for item in page.items {
                            match item {
                                PlaylistItem {
                                    added_at: Some(added_at),
                                    item: Some(PlayableItem::Track(item)),
                                    ..
                                } => {
                                    let entry = match ratings.iter_mut().find_map(
                                        |(s_track, analyzation)| {
                                            (*s_track == item).then_some(analyzation)
                                        },
                                    ) {
                                        Some(ratings) => ratings,
                                        None => {
                                            &mut ratings
                                                .push_mut((item, TrackAnalyzation::default()))
                                                .1
                                        }
                                    };

                                    entry.rating_history.push((
                                        UtcDateTime::from_unix_timestamp(added_at.timestamp())
                                            .unwrap(),
                                        rating,
                                    ));
                                }
                                other => {
                                    eprintln!(
                                        "Unexpected format for rating playlist entry: {other:?}"
                                    )
                                }
                            }
                        }
                    }
                    Err(e) => eprintln!("Failed to get playlist item page: {e}"),
                }
            }
        }

        analyze(ratings)
    },
    RATINGS,
    Duration::minutes(1)
);

caching!(
    playback_state,
    Option<CurrentPlaybackContext>,
    async |_previous| {
        println!("Getting playback state");

        let spotify = spotify().await;
        retrying(
            move |_| async move { spotify.current_playback(None, None::<[_; 0]>).await },
            (),
        )
        .await
        .ok()
        .flatten()
    },
    PLAYBACK_STATE,
    Duration::seconds(2)
);

// TODO: maybe return None if there are no ratings yet and display that in the ui
#[server]
pub async fn rating(track_id: TrackId<'static>) -> Result<f32> {
    let ratings = ratings_server().await;

    Ok(ratings
        .tracks
        .iter()
        .find(|(track, _)| track.id.as_ref() == Some(&track_id))
        .map(|(_, analyzation)| analyzation.canonical_rating)
        .unwrap_or(DEFAULT_RATING))
}
