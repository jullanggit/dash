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
    CurrentPlaybackContext, FullTrack, PlayableItem, PlaylistItem, SimplifiedPlaylist, TrackId,
};
#[cfg(feature = "server")]
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, env::home_dir, sync::OnceLock};
use time::{Date, Duration, UtcDateTime};
#[cfg(feature = "server")]
use tokio::{
    fs,
    sync::{Mutex, RwLock},
    time::{Duration as TokioDuration, sleep},
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

/// Return the result of `f`, caching to memory and disk, updating ever `interval`.
///
/// `last_fetched` serves as synchronization and interval control: whenever a request is updating the value, it locks the mutex, and updates the datetime inside.
/// The `in_mem_cache` and `last_fetched` are separated, to allow for quick cache retrieval even while the value is being updated.
/// `in_mem_cache` is only None before the first initialization.
#[cfg(feature = "server")]
async fn refreshing<T, F, Fut>(
    f: F,
    in_mem_cache: &'static RwLock<Option<T>>,
    last_fetched: &'static Mutex<UtcDateTime>,
    interval: time::Duration,
    name: &str,
) -> T
where
    T: Clone + Serialize + DeserializeOwned + Send + Sync,
    F: Fn(Option<T>) -> Fut + Send + 'static,
    Fut: Future<Output = T> + Send + 'static,
{
    let now = UtcDateTime::now();

    let read_mem_cache = async || in_mem_cache.read().await.clone();

    let in_mem_cached = read_mem_cache().await;
    let needs_update = last_fetched
        .try_lock()
        .map(|last_fetched| (now > *last_fetched + interval, last_fetched));
    match (in_mem_cached, needs_update) {
        // there is a cached value, and it doesn't need updating
        (Some(cached), Ok((false, _))) | (Some(cached), Err(_)) => cached,
        // no other request is currently updating the value, and it needs updating; update it
        (in_mem_cached, Ok((true, guard))) => {
            let write_mem_cache = async move |value: T| *in_mem_cache.write().await = Some(value);

            let disk_cache_path = home_dir().map(|mut path| {
                path.push(format!(".cache/dash/{name}.json"));
                path
            });
            let disk_cache_path_clone = disk_cache_path.clone();
            let write_mem_and_disk_cache = async move |value: T| {
                write_mem_cache(value.clone()).await;
                let path = disk_cache_path_clone?;
                fs::create_dir_all(path.parent()?).await.ok()?;
                fs::write(path, serde_json::to_string(&value).ok()?)
                    .await
                    .ok()
            };

            match in_mem_cached {
                // fetch and write new value to cache in the background
                Some(cached) => {
                    let clone = cached.clone();
                    tokio::spawn(async move {
                        write_mem_and_disk_cache(f(Some(clone)).await).await;

                        // hold lock until all caches are updated
                        drop(guard);
                    });

                    cached
                }
                None => {
                    let read_disk_cache = async || -> Option<T> {
                        serde_json::from_str(
                            &fs::read_to_string(disk_cache_path.clone()?).await.ok()?,
                        )
                        .ok()
                    };
                    match read_disk_cache().await {
                        // update asynchronously, return cached value
                        Some(cached) => {
                            write_mem_cache(cached.clone()).await;

                            let clone = cached.clone();
                            tokio::spawn(async move {
                                write_mem_and_disk_cache(f(Some(clone)).await).await;

                                // hold lock until all caches are updated
                                drop(guard);
                            });

                            cached
                        }
                        // update synchronously, return new value once its available
                        None => {
                            let new_value = f(None).await;
                            write_mem_and_disk_cache(new_value.clone()).await;

                            // hold lock until all caches are updated
                            drop(guard);

                            new_value
                        }
                    }
                }
            }
        }
        // cache not yet initialized, but another request is currently doing so; wait for that to complete
        (None, Err(_)) => loop {
            if let Some(value) = read_mem_cache().await {
                return value;
            }

            sleep(TokioDuration::from_millis(100)).await;
        },
        (None, Ok((false, _))) => {
            panic!("If the value doesn't need updating, there should be a cached value")
        }
    }
}

macro_rules! refreshing {
    ($fn_name:ident, $return:ty, $closure:expr, $const:ident, $interval:expr) => {
        #[cfg(feature = "server")]
        static $const: RwLock<Option<$return>> = RwLock::const_new(None);
        #[cfg(feature = "server")]
        static ${ concat($const, _LAST_FETCH) }: Mutex<UtcDateTime> = Mutex::const_new(UtcDateTime::MIN); // initialize to min so the first access is always identified as after it

        #[cfg(feature = "server")]
        pub async fn ${ concat($fn_name, _server) }() -> $return {
            refreshing($closure, &$const, &${ concat($const, _LAST_FETCH) }, $interval, stringify!($fn_name)).await
        }

        /// Always returns Ok(value) on the server
        #[server]
        pub async fn $fn_name() -> Result<$return> {
            Ok(${ concat($fn_name, _server) }().await)
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
            use_interval(std::time::Duration::from_nanos($interval.whole_nanoseconds() as u64), move |_| body());

            state
        }
    };
}
refreshing!(
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
    Duration::seconds(1)
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
        let page = f(offset).await;
        let mut end = false;
        // update housekeeping on Ok, retry on too many requests Err, pass anything else on to `callback`
        match page {
            Ok(ref page) => {
                offset += page.items.len() as u32;
                end = page.next.is_none();
            }
            Err(ClientError::Http(ref http)) => match http.as_ref() {
                rspotify_http::HttpError::StatusCode(response) => {
                    let code = response.status().as_u16();
                    if code == 429 {
                        let retry_after = response
                            .headers()
                            .iter()
                            .find(|(name, _)| name.as_str() == "retry-after")
                            .and_then(|(_, value)| value.to_str().ok())
                            .map(|str| str.parse().unwrap_or(60));
                        if let Some(after) = retry_after {
                            // wait for retry-after, retry in the next loop, as offset didnt get incremented
                            println!("Retrying {} after {after} seconds", response.url());
                            sleep(std::time::Duration::from_secs(after)).await;
                            continue;
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }

        out.push(page);
        if end {
            break;
        }
    }
    out
}

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
    async |_previous| {
        let spotify = spotify().await;
        let playlists = rating_playlists()
            .await
            .expect("Never errors on server-to-server calls");
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
    Duration::seconds(1)
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
    async |_previous| {
        let spotify = spotify().await;
        spotify
            .current_playback(None, None::<[_; 0]>)
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
    let ratings = ratings().await.expect("Server-to-server");

    Ok(ratings
        .tracks
        .iter()
        .find(|(track, _)| track.id.as_ref() == Some(&track_id))
        .map(|(_, analyzation)| analyzation.canonical_rating)
        .unwrap_or(DEFAULT_RATING))
}
