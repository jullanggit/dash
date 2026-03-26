#[cfg(feature = "server")]
use crate::spotify::caching::{caching, caching_hashmap};
use crate::{
    caching, caching_hashmap,
    spotify::{
        analyze::{Analyzation, TrackAnalyzation},
        caching::use_server_fn,
    },
};
#[cfg(feature = "server")]
use dashmap::DashMap;
use dioxus::prelude::*;
#[cfg(feature = "server")]
use futures::Stream;
use futures::StreamExt;
#[cfg(feature = "server")]
use rspotify::{
    AuthCodeSpotify, ClientError, ClientResult, Config, Credentials, OAuth,
    prelude::{BaseClient, OAuthClient},
    scopes,
};
#[cfg(feature = "server")]
use rspotify_model::Page;
use rspotify_model::{
    ArtistId, CurrentPlaybackContext, FullArtist, FullTrack, PlayableItem, PlaylistId,
    PlaylistItem, SavedTrack, SimplifiedArtist, SimplifiedPlaylist, TrackId,
};
#[cfg(feature = "server")]
use serde::de::DeserializeOwned;
#[cfg(feature = "server")]
use std::pin::Pin;
use std::{
    collections::HashSet,
    sync::{Arc, LazyLock, OnceLock},
};
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
    trace!("Getting spotify");

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

        trace!("Getting rating playlists");

        let mut response = paginate_retrying(move |offset| {
            let spotify = spotify.clone();
            async move {
                trace!("[SPOTIFY API LOG] current user playlists, offset {offset}");
                spotify
                    .current_user_playlists_manual(None, Some(offset))
                    .await
            }
        })
        .await;

        while let Some(result) = response.next().await {
            match result {
                Ok(playlist) => {
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
                Err(e) => error!("Error getting playlists: {e}"),
            }
        }

        playlists
    },
    RATING_PLAYLISTS,
    Duration::minutes(5)
);

/// Paginates the given function, retrying any too-many-request errors.
/// Returns early if any other errors are encountered.
#[cfg(feature = "server")]
async fn paginate_retrying<F, Fut, T>(f: F) -> Pin<Box<impl Stream<Item = ClientResult<T>>>>
where
    F: Fn(u32) -> Fut,
    Fut: Future<Output = ClientResult<Page<T>>>,
    T: DeserializeOwned,
{
    let mut offset = 0;
    Box::pin(async_stream::stream! {
        loop {
            let page = retrying(&f, offset).await?;

            offset += page.items.len() as u32;
            let end = page.next.is_none() || page.items.is_empty();

            for item in page.items {
                yield Ok(item);
            }

            if end {
                break;
            };
        }
    })
}

/// Retries `f` if it got a too-many-requests error.
/// Suspends for the duration requested by the spotify API, which may be a long period of time.
#[cfg(feature = "server")]
pub async fn retrying<F, Args, Fut, T>(f: F, args: Args) -> ClientResult<T>
where
    F: Fn(Args) -> Fut,
    Args: Clone,
    Fut: Future<Output = ClientResult<T>>,
    T: DeserializeOwned,
{
    let mut num_tries = 0;
    loop {
        let res = f(args.clone()).await;
        if let Err(ClientError::Http(ref http)) = res
            && let rspotify_http::HttpError::StatusCode(response) = http.as_ref()
            && num_tries <= 5
        {
            let retry_after = response
                .status()
                .as_u16()
                .eq(&429)
                .then(|| {
                    response
                        .headers()
                        .iter()
                        .find(|(name, _)| name.as_str() == "retry-after")
                        .and_then(|(_, value)| value.to_str().ok())
                        .and_then(|str| str.parse().ok())
                })
                .flatten()
                .unwrap_or_else(|| {
                    num_tries += 1;
                    2u64.pow(num_tries - 1)
                });

            // wait for retry-after, retry in the next loop, as offset didnt get incremented
            info!("Retrying {} after {retry_after} seconds", response.url());
            sleep(std::time::Duration::from_secs(retry_after)).await;
            continue;
        }
        return res;
    }
}

// TODO: also search from the back for new items
caching!(
    ratings,
    Analyzation,
    // get ratings. Only re-fetch ratings within the last 15 minutes.
    async |previous| {
        use crate::spotify::analyze::analyze;

        let spotify = spotify().await;
        let playlists = rating_playlists_server().await;
        let mut ratings = previous.unwrap_or_default().tracks;

        // remove any ratings younger than 15 minutes
        let now = UtcDateTime::now();
        ratings.retain_mut(|(_, analyzation)| {
            analyzation
                .rating_history
                .retain(|(date_time, _)| now - Duration::minutes(15) > *date_time);
            !analyzation.rating_history.is_empty()
        });

        trace!("Getting ratings");

        for (rating, playlist) in playlists {
            let mut items = paginate_retrying(move |offset| {
                let spotify = spotify.clone();
                let id = playlist.id.clone();
                async move {
                    trace!("[SPOTIFY API LOG] playlist items, id {id}, offset {offset}");
                    spotify
                        .playlist_items_manual(id, None, None, None, Some(offset))
                        .await
                }
            })
            .await;

            // assumptions:
            // The first initialization fetches all available items.
            // Items older than 15 minutes do not change and are not removed. This can be extended to 'no items are ever changed or removed' once all logic has converted to append-only.
            while let Some(result) = items.next().await {
                match result {
                    Ok(item) => match item {
                        PlaylistItem {
                            added_at: Some(added_at),
                            item: Some(PlayableItem::Track(item)),
                            ..
                        } => {
                            let entry =
                                match ratings.iter_mut().find_map(|(s_track, analyzation)| {
                                    (*s_track == item).then_some(analyzation)
                                }) {
                                    Some(ratings) => ratings,
                                    None => {
                                        &mut ratings.push_mut((item, TrackAnalyzation::default())).1
                                    }
                                };

                            let data = (
                                UtcDateTime::from_unix_timestamp(added_at.timestamp()).unwrap(),
                                rating,
                            );

                            if entry.rating_history.contains(&data) {
                                // We have arrived at data older than 15 minutes we already have, and which we assume hasn't changed, so we stop fetching here.
                                break;
                            } else {
                                entry.rating_history.push(data);
                            }
                        }
                        other => {
                            error!("Unexpected format for rating playlist entry: {other:?}")
                        }
                    },
                    Err(e) => error!("Failed to get playlist items: {e}"),
                }
            }
        }

        analyze(ratings).await
    },
    RATINGS,
    Duration::seconds(10)
);

caching!(
    saved_tracks,
    HashSet<TrackId<'static>>,
    // get ratings. Only re-fetch ratings within the last 15 minutes.
    async |previous| {
        let spotify = spotify().await;
        let mut saved_tracks = previous.unwrap_or_default();

        trace!("Getting saved tracks");

        let mut items = paginate_retrying(move |offset| {
            let spotify = spotify.clone();
            async move {
                trace!("[SPOTIFY API LOG] saved_tracks, offset {offset}");
                spotify
                    .current_user_saved_tracks_manual(None, None, Some(offset))
                    .await
            }
        })
        .await;

        // assumptions:
        // The first initialization fetches all available items.
        // The saved tracks is only ever appended to. TODO: refetch the entire playlist from time to time
        while let Some(result) = items.next().await {
            match result {
                Ok(SavedTrack { track, .. }) => {
                    if let FullTrack {
                        id: Some(track_id), ..
                    } = track
                        && !saved_tracks.insert(track_id)
                    {
                        break;
                    }
                }
                Err(e) => error!("Failed to get saved tracks: {e}"),
            }
        }

        saved_tracks
    },
    SAVED_TRACKS,
    Duration::minutes(1)
);

caching!(
    playback_state,
    Option<CurrentPlaybackContext>,
    async |_previous| {
        trace!("Getting playback state");

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
    Duration::seconds(1)
);

caching!(
    queue,
    Vec<PlayableItem>,
    async |_| {
        trace!("Getting queue");

        let spotify = spotify().await;
        retrying(
            move |_| async move { spotify.current_user_queue().await },
            (),
        )
        .await
        .map(|queue| queue.queue)
        .unwrap_or_default()
    },
    QUEUE,
    Duration::seconds(1)
);

// TODO: maybe return None if there are no ratings yet and display that in the ui
#[server]
pub async fn rating(track_id: TrackId<'static>) -> Result<f32> {
    Ok(ratings_server().await.rating(track_id))
}

caching_hashmap!(
    full_artist,
    ArtistId<'static>,
    FullArtist,
    async |artist_id, _| {
        info!("Getting artist with id {artist_id}");

        let spotify = spotify().await;

        match retrying(
            move |artist_id| async move { spotify.artist(artist_id).await },
            artist_id.clone(),
        )
        .await
        {
            Ok(artist) => artist,
            Err(e) => panic!("Failed to get artist {}: {e}", artist_id),
        }
    },
    ARTISTS,
    Duration::weeks(4) // assume artists are mostly static
);

pub async fn genres(artists: &[SimplifiedArtist]) -> HashSet<String> {
    let mut genres = HashSet::new();

    for artist in artists.iter() {
        if let Some(ref artist_id) = artist.id {
            let full_artist = match full_artist(artist_id.clone()).await {
                Ok(artist) => artist,
                Err(e) => {
                    error!("Failed to fetch artist {}: {e}", artist.name);
                    continue;
                }
            };
            for genre in full_artist.genres {
                genres.insert(genre.clone());
            }
        }
    }

    genres
}

caching_hashmap!(
    playlist_tracks,
    PlaylistId<'static>,
    Vec<FullTrack>,
    async |playlist_id, _| {
        let spotify = spotify().await;

        let mut out = Vec::new();
        let mut items = paginate_retrying(move |offset| {
            let spotify = spotify.clone();
            let id = playlist_id.clone();
            async move {
                trace!("[SPOTIFY API LOG] playlist items, id {id}, offset {offset}");
                spotify
                    .playlist_items_manual(id, None, None, None, Some(offset))
                    .await
            }
        })
        .await;

        while let Some(result) = items.next().await {
            match result {
                Ok(item) => match item {
                    PlaylistItem {
                        item: Some(PlayableItem::Track(track)),
                        ..
                    } => out.push(track),
                    other => {
                        info!("Non-track playlist entry: {other:?}")
                    }
                },
                Err(e) => error!("Failed to get playlist items: {e}"),
            }
        }

        out
    },
    PLAYLIST_ITEMS,
    Duration::HOUR
);

// (mis)use caching macro to periodically serialize and automatically deserialize
caching!(
    weighted_playback_enabled,
    HashSet<PlaylistId<'static>>,
    async |previous| { previous.unwrap_or_default() },
    WEIGHTED_PLAYBACK_ENABLED,
    Duration::weeks(52)
);
#[server]
pub async fn weighted_playback(playlist: PlaylistId<'static>, enabled: bool) -> Result<()> {
    let mut option = WEIGHTED_PLAYBACK_ENABLED.write().await;
    let set = option.get_or_insert_with(HashSet::new);
    if enabled {
        set.insert(playlist);
    } else {
        set.remove(&playlist);
    }

    Ok(())
}
