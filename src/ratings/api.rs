use futures::StreamExt;
use rspotify::{
    AuthCodeSpotify, Config, Credentials, OAuth,
    model::{FullTrack, PlayableItem, PlaylistItem, SimplifiedPlaylist},
    prelude::{BaseClient, OAuthClient},
    scopes,
};
use std::{
    collections::BTreeMap,
    sync::{
        OnceLock,
        atomic::{AtomicI64, Ordering},
    },
};
use time::{Date, Duration, UtcDateTime};
use tokio::sync::RwLock;

static SPOTIFY: OnceLock<AuthCodeSpotify> = OnceLock::new();

// TODO: read credentials from config file (possibly with indirection for secrets) instead form .env
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
                tokio::spawn(async move { *$const.write().await = Some(clone); }); // update in the background

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

refreshing!(
    rating_playlists,
    Vec<(f32, SimplifiedPlaylist)>,
    {
        let spotify = spotify().await;
        let mut playlists = Vec::new();

        let mut stream = spotify.current_user_playlists();
        while let Some(playlist) = stream.next().await {
            if let Ok(playlist) = playlist
                && let Ok(rating) = playlist.name.parse::<f32>()
                && (0.0..=5.0).contains(&rating)
            {
                if playlists.iter().any(|(s_rating, _)| *s_rating == rating) {
                    panic!("Rating folder already present")
                } else {
                    playlists.push((rating, playlist.clone()))
                };
            }
        }
        playlists
    },
    RATING_PLAYLISTS
);

/// Contains all analyzations derived from `rating_history` and the providing track
#[derive(Clone, Debug, Default)]
pub struct TrackAnalyzation {
    /// sorted by ascending date
    pub rating_history: Vec<(UtcDateTime, f32)>,
    pub canonical_rating_history: Vec<(UtcDateTime, f32)>,
    pub canonical_rating: f32,
}

#[derive(Clone, Debug)]
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
    {
        let spotify = spotify().await;
        let playlists = rating_playlists().await;
        let mut ratings = Vec::new();

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
    RATINGS
);

/// Build analyzation based on tracks and rating histories
fn analyze(mut tracks: AnalyzedTracks) -> Analyzation {
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
            .unwrap_or(2.5); // TODO: make default rating configurable
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
