use dioxus::prelude::*;
use dioxus_sdk_time::use_interval;
#[cfg(feature = "server")]
use futures::StreamExt;
#[cfg(feature = "server")]
use rspotify::{
    AuthCodeSpotify, ClientError, Config, Credentials, OAuth,
    prelude::{BaseClient, OAuthClient},
    scopes,
};
use rspotify_model::{
    CurrentPlaybackContext, FullTrack, PlayableItem, PlaylistItem, SimplifiedPlaylist, TrackId,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    env::home_dir,
    sync::{
        OnceLock,
        atomic::{AtomicI64, Ordering},
    },
};
use time::{Date, Duration, UtcDateTime};
#[cfg(feature = "server")]
use tokio::{fs, sync::RwLock, time::sleep};

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

macro_rules! refreshing {
    ($fn_name:ident, $return:ty, $body:expr, $const:ident, $interval_millis:literal) => {
        #[cfg(feature = "server")]
        static $const: RwLock<Option<$return>> = RwLock::const_new(None);
        #[cfg(feature = "server")]
        static ${ concat($const, _LAST_FETCH) }: AtomicI64 = AtomicI64::new(0); // initialize to zero so the first access is always identified as after it

        /// Always returns Ok(value) on the server
        #[server]
        pub async fn $fn_name() -> Result<$return> {
            let now = UtcDateTime::now();

            let read_clone = async || -> Option<$return> { $const.read().await.clone() };

            let last_fetched = ${ concat($const, _LAST_FETCH) }.load(Ordering::Relaxed);
            if (now - time::Duration::milliseconds($interval_millis)).unix_timestamp() > last_fetched
                && ${ concat($const, _LAST_FETCH) }
                    .compare_exchange(
                        last_fetched,
                        now.unix_timestamp(),
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    )
                    .is_ok()
            {
                let function = $body;

                let cache_path = home_dir().map(|mut path| {
                    path.push(format!(".cache/dash/{}.json", stringify!($fn_name)));
                    path
                });

                let clone = cache_path.clone();

                let write = async move |value: $return| *$const.write().await = Some(value.clone());
                let write_with_cache = async move |value: $return| {
                    write(value.clone()).await;
                    let path = clone?;
                    fs::create_dir_all(path.parent()?).await.ok()?;
                    fs::write(path, serde_json::to_string(&value).ok()?).await.ok()
                };

                match read_clone().await {
                    // update asynchronously, return old value
                    Some(value) => {
                        let clone = value.clone();
                        tokio::spawn(async move { write_with_cache(function(Some(clone)).await).await; });
                        Ok(value)
                    }
                    None => {
                        let get_cached = async || -> Option<$return> {
                            serde_json::from_str(&fs::read_to_string(cache_path.clone()?).await.ok()?).ok()
                        };
                        match get_cached().await {
                            // update asynchronously, return cached value
                            Some(cached) => {
                                let clone = cached.clone();
                                tokio::spawn(async move { write_with_cache(function(Some(clone)).await).await; });

                                write(cached.clone()).await;
                                Ok(cached)
                            }
                            // update synchronously, return new value once its available
                            None => {
                                let new_value = function(None).await;
                                write_with_cache(new_value.clone()).await;
                                Ok(new_value)
                            }
                        }
                    }
                }
            // up-to-date, or being initialized/updated by another thread
            } else {
                loop {
                    if let Some(value) = read_clone().await {
                        return Ok(value);
                    }

                    sleep(tokio::time::Duration::from_millis(100)).await;
                }
            }
        }

        pub fn ${ concat(use_, $fn_name) }() -> Signal<Option<$return>> {
            let mut state = use_signal(|| None);

            let body = move || async move {
                let new_state = $fn_name().await;

                if let Ok(new_state) = new_state && state.read().as_ref() != Some(&new_state) {
                    state.set(Some(new_state));
                }
            };

            use_future(move || body());
            use_interval(std::time::Duration::from_millis($interval_millis), move |_| body());

            state
        }
    };
}
refreshing!(
    rating_playlists,
    Vec<(f32, SimplifiedPlaylist)>,
    async |previous| {
        let spotify = spotify().await;
        let mut playlists = Vec::new();

        println!("Getting rating playlists");

        let mut callback = |playlist: SimplifiedPlaylist| {
            if let Ok(rating) = playlist.name.parse::<f32>()
                && (0.0..=5.0).contains(&rating)
            {
                if playlists.iter().any(|(s_rating, _)| *s_rating == rating) {
                    panic!("Rating folder already present")
                } else {
                    playlists.push((rating, playlist.clone()))
                };
            }
        };

        let mut offset = 0;
        loop {
            let page = spotify
                .current_user_playlists_manual(None, Some(offset))
                .await;
            // retry on too many requests error, log and ignore all others
            match page {
                Ok(page) => {
                    offset += page.items.len() as u32;
                    for item in page.items {
                        callback(item);
                    }
                    if page.next.is_none() {
                        break;
                    }
                }
                Err(ClientError::Http(http)) => match *http {
                    rspotify_http::HttpError::StatusCode(response) => {
                        let code = response.status().as_u16();
                        if code == 429 {
                            let retry_after = response
                                .headers()
                                .iter()
                                .find(|(name, value)| name.as_str() == "retry-after")
                                .and_then(|(_, value)| value.to_str().ok())
                                .map(|str| str.parse().unwrap_or(60));
                            if let Some(after) = retry_after {
                                // wait for retry-after, retry in the next loop, as offset didnt get incremented
                                println!("Retrying after {after} seconds");
                                sleep(std::time::Duration::from_secs(after)).await;
                            }
                        }
                    }
                    other => eprintln!("Error getting page: {other}"),
                },
                Err(other) => eprintln!("Error getting page: {other}"),
            }
        }

        playlists
    },
    RATING_PLAYLISTS,
    1000
);

/// Contains all analyzations derived from `rating_history` and the providing track
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct TrackAnalyzation {
    /// sorted by ascending date
    pub rating_history: Vec<(UtcDateTime, f32)>,
    pub canonical_rating_history: Vec<(UtcDateTime, f32)>,
    pub canonical_rating: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Analyzation {
    pub tracks: AnalyzedTracks,
    /// sorted by ascending date
    pub average_rating_per_day: Vec<(Date, f32)>,
    pub num_ratings_history: Vec<(UtcDateTime, u32)>,
    pub num_rated_tracks_history: Vec<(UtcDateTime, u32)>,
}

pub type AnalyzedTracks = Vec<(FullTrack, TrackAnalyzation)>;

refreshing!(
    ratings,
    Analyzation,
    async |previous| {
        let spotify = spotify().await;
        let playlists = rating_playlists()
            .await
            .expect("Never errors on server-to-server calls");
        let mut ratings = Vec::new();

        println!("Getting ratings");

        for (rating, playlist) in playlists {
            let mut stream = spotify.playlist_items(playlist.id, None, None);
            while let Some(item) = stream.next().await {
                match item {
                    Ok(PlaylistItem {
                        added_at: Some(added_at),
                        track: Some(PlayableItem::Track(track)),
                        ..
                    }) => {
                        let entry = match ratings.iter_mut().find_map(|(s_track, analyzation)| {
                            (*s_track == track).then_some(analyzation)
                        }) {
                            Some(ratings) => ratings,
                            None => &mut ratings.push_mut((track, TrackAnalyzation::default())).1,
                        };

                        entry.rating_history.push((
                            UtcDateTime::from_unix_timestamp(added_at.timestamp()).unwrap(),
                            rating,
                        ));
                    }
                    other => eprintln!("Unexpected format for rating playlist entry: {other:?}"),
                }
            }
        }

        analyze(ratings)
    },
    RATINGS,
    1000
);

// TODO: make this configurable
const DEFAULT_RATING: f32 = 2.5;

/// Build analyzation based on tracks and rating histories
fn analyze(mut tracks: AnalyzedTracks) -> Analyzation {
    println!("Analyzing ratings");

    fn canonical_rating(rating_history: impl IntoIterator<Item = (f32, UtcDateTime)>) -> f32 {
        const HALF_LIFE: Duration = Duration::weeks(26);

        let now = UtcDateTime::now();
        let (weighted_sum, weight_sum) = rating_history.into_iter().fold(
            (0., 0.),
            |(weighted_sum, weight_sum), (rating, time)| {
                let delta = now - time;
                let weight = 0.5_f64.powf(delta / HALF_LIFE) as f32;

                (weighted_sum + rating * weight, weight_sum + weight)
            },
        );

        weighted_sum / weight_sum
    }

    // track analyzations
    for (_, analyzation) in &mut tracks {
        analyzation
            .rating_history
            .sort_unstable_by_key(|&(time, _)| time);

        analyzation.canonical_rating_history = (1..=analyzation.rating_history.len())
            .map(|i| {
                (
                    analyzation.rating_history[i - 1].0,
                    canonical_rating(
                        analyzation
                            .rating_history
                            .iter()
                            .take(i)
                            .map(|&(time, rating)| (rating, time)),
                    ),
                )
            })
            .collect();
        analyzation.canonical_rating = analyzation
            .canonical_rating_history
            .last()
            .map(|(_, rating)| *rating)
            .unwrap_or(DEFAULT_RATING);
    }

    // cross-track analyzations
    let average_rating_per_day = {
        let ratings_per_day: BTreeMap<Date, Vec<f32>> = tracks
            .iter()
            .flat_map(|(_, track_analyzation)| track_analyzation.rating_history.iter())
            .fold(BTreeMap::new(), |mut acc, (date_time, rating)| {
                let date = date_time.date();
                acc.entry(date).or_default().push(*rating);
                acc
            });

        ratings_per_day
            .iter()
            .map(|(&date, ratings)| {
                let average_rating =
                    ratings.iter().map(f32::clone).sum::<f32>() / ratings.len() as f32;
                (date, average_rating)
            })
            .collect()
    };

    let (num_ratings_history, num_rated_tracks_history) = {
        let rating_times = tracks
            .iter()
            .flat_map(|(_, data)| data.rating_history.iter().map(|(time, _)| time))
            .collect::<Vec<_>>();

        let first_rating_times = tracks
            .iter()
            .filter_map(|(_, data)| data.rating_history.iter().map(|(time, _)| time).min())
            .collect::<Vec<_>>();

        let history = |mut times: Vec<&UtcDateTime>| {
            times.sort_unstable();
            times
                .iter()
                .enumerate()
                .map(|(count, &&date_time)| (date_time, count as u32))
                .collect()
        };

        (history(rating_times), history(first_rating_times))
    };

    Analyzation {
        tracks,
        average_rating_per_day,
        num_ratings_history,
        num_rated_tracks_history,
    }
}

refreshing!(
    playback_state,
    Option<CurrentPlaybackContext>,
    async |previous| {
        let spotify = spotify().await;
        spotify
            .current_playback(None, None::<[_; 0]>)
            .await
            .ok()
            .flatten()
    },
    PLAYBACK_STATE,
    2000
);

// TODO: maybe return None if there are no ratings yet and display that in the ui
#[server]
pub async fn rating(track_id: TrackId<'static>) -> Result<f32> {
    let ratings = ratings().await.expect("Server-to-server");

    Ok(ratings
        .tracks
        .iter()
        .find(|(track, _)| track.id.as_ref() == Some(&track_id))
        .map(|(_, analyzation)| analyzation.canonical_rating)
        .unwrap_or(DEFAULT_RATING))
}
