#[cfg(feature = "server")]
use crate::spotify::caching::Cache;
use crate::{
    caching, caching_hashmap,
    spotify::{
        analyze::{Analyzation, RATING_OVERWRITE_WINDOW, TrackAnalyzation},
        caching::use_server_fn,
        playback::{PlaybackOptions, PlaybackSelection},
    },
};
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
    ArtistId, CurrentPlaybackContext, FullArtist, FullTrack, PlayHistory, PlayableId, PlayableItem,
    PlaylistId, PlaylistItem, PlaylistTracksRef, PrivateUser, SavedTrack, SimplifiedArtist,
    SimplifiedPlaylist, TrackId,
};
#[cfg(feature = "server")]
use serde::de::DeserializeOwned;
use std::{collections::HashSet, iter, sync::OnceLock};
#[cfg(feature = "server")]
use std::{fmt::Debug, pin::Pin};
use time::{Duration, UtcDateTime};
#[cfg(feature = "server")]
use tokio::time::sleep;

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
                        "playlist-read-collaborative",
                        "playlist-modify-private",
                        "playlist-modify-public",
                        "user-library-read",
                        "user-read-currently-playing",
                        "user-read-playback-state",
                        "user-modify-playback-state",
                        "user-read-recently-played"
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
    |_, _| async move {
        let spotify = spotify().await;
        let mut playlists: Vec<(f32, SimplifiedPlaylist)> = Vec::new();

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
            let playlist = result.context("Error getting playlist")?;
            if let Ok(rating) = playlist.name.parse::<f32>()
                && (0.0..=5.0).contains(&rating)
            {
                if playlists.iter().any(|(s_rating, s_playlist)| {
                    *s_rating == rating && s_playlist.id != playlist.id
                }) {
                    return Err(anyhow::anyhow!("Duplicate rating folder {rating}"));
                } else {
                    playlists.push((rating, playlist.clone()))
                };
            }
        }

        Ok(playlists)
    },
    RATING_PLAYLISTS,
    Duration::minutes(3)
);

/// Paginates the given function, retrying any too-many-request errors.
/// Returns early if any other errors are encountered.
#[cfg(feature = "server")]
async fn paginate_retrying<F, Fut, T>(f: F) -> Pin<Box<impl Stream<Item = ClientResult<T>>>>
where
    F: Fn(u32) -> Fut,
    Fut: Future<Output = ClientResult<Page<T>>>,
    T: DeserializeOwned + Debug,
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
    T: DeserializeOwned + Debug,
{
    let mut num_tries = 0;
    loop {
        let res = f(args.clone()).await;
        if let Err(ClientError::Http(ref http)) = res
            && let rspotify_http::HttpError::StatusCode(response) = http.as_ref()
            && num_tries <= 5
        {
            let status_code = response.status().as_u16();
            let retry_after = status_code
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
            info!(
                "Retrying {} after {retry_after} seconds: {res:?}",
                response.url()
            );
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
    |_, previous| async move {
        use crate::spotify::analyze::analyze;
        use std::collections::HashMap;

        let spotify = spotify().await.clone();
        let playlists = &rating_playlists_server().await.value;
        let mut previous = previous.unwrap_or_default().value.clone();
        let previous_snapshot_ids = previous.playlist_snapshot_ids;
        let mut ratings = previous.tracks;

        trace!("Getting ratings");

        let current_snapshot_ids = playlists
            .iter()
            .map(|(_, playlist)| (playlist.id.clone(), playlist.snapshot_id.clone()))
            .collect::<HashMap<_, _>>();

        for (rating, playlist) in playlists.iter() {
            if previous_snapshot_ids
                .get(&playlist.id)
                .is_some_and(|previous| *previous == playlist.snapshot_id)
            {
                continue;
            }

            let spotify_clone = spotify.clone();
            let mut items = paginate_retrying(move |offset| {
                let spotify = spotify_clone.clone();
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
                let item = result.context("Failed to get playlist item")?;
                match item {
                    PlaylistItem {
                        added_at: Some(added_at),
                        item: Some(PlayableItem::Track(item)),
                        ..
                    } => {
                        let entry = match ratings.iter_mut().find_map(|(s_track, analyzation)| {
                            (*s_track == item).then_some(analyzation)
                        }) {
                            Some(ratings) => ratings,
                            None => &mut ratings.push_mut((item, TrackAnalyzation::default())).1,
                        };

                        let data = (
                            UtcDateTime::from_unix_timestamp(added_at.timestamp()).unwrap(),
                            *rating,
                        );

                        if entry.rating_history.contains(&data) {
                            // We have arrived at data older than 15 minutes we already have, and which we assume hasn't changed, so we stop fetching here.
                            break;
                        } else {
                            entry.rating_history.push(data);
                        }
                    }
                    other => {
                        return Err(anyhow::anyhow!(
                            "Unexpected format for rating playlist entry: {other:?}"
                        ));
                    }
                }
            }
        }

        let mut analyzation = analyze(ratings).await;
        analyzation.playlist_snapshot_ids = current_snapshot_ids;
        Ok(analyzation)
    },
    RATINGS,
    Duration::minutes(3)
);

caching!(
    saved_tracks,
    HashSet<TrackId<'static>>,
    // get ratings. Only re-fetch ratings within the last 15 minutes.
    |_, previous| async move {
        let spotify = spotify().await;
        let mut saved_tracks = previous
            .map(|previous| previous.value.clone())
            .unwrap_or_default();

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
            let SavedTrack { track, .. } = result.context("Failed to get saved track")?;
            if let FullTrack {
                id: Some(track_id), ..
            } = track
                && !saved_tracks.insert(track_id)
            {
                break;
            }
        }

        Ok(saved_tracks)
    },
    SAVED_TRACKS,
    Duration::minutes(1)
);

caching!(
    playback_state,
    Option<CurrentPlaybackContext>,
    |_, _| async move {
        trace!("Getting playback state");

        let spotify = spotify().await;

        Ok(retrying(
            move |_| async move { spotify.current_playback(None, None::<[_; 0]>).await },
            (),
        )
        .await?)
    },
    PLAYBACK_STATE,
    Duration::seconds(1)
);

caching!(
    queue,
    Vec<PlayableItem>,
    |_, _| async move {
        trace!("Getting queue");

        let spotify = spotify().await;
        Ok(retrying(
            move |_| async move { spotify.current_user_queue().await },
            (),
        )
        .await?
        .queue)
    },
    QUEUE,
    Duration::seconds(1)
);
#[cfg(feature = "server")]
pub async fn add_to_queue(track: TrackId<'static>) -> Result<(), anyhow::Error> {
    use rspotify_model::{SimplifiedAlbum, Type};
    use std::collections::HashMap;

    let spotify = spotify().await;
    let res = retrying(
        move |(spotify, track)| async move { spotify.add_item_to_queue(track.into(), None).await },
        (spotify, track.clone()),
    )
    .await;
    if let Err(e) = res {
        Err(anyhow::anyhow!("Failed to add track {track} to queue: {e}"))
    } else {
        use std::sync::Arc;

        let mut with_last_fetched =
            Arc::map(QUEUE.read_cache(&()).await.unwrap_or_default(), |wlf| {
                let mut wlf = wlf.clone();
                wlf.value.insert(
                    0,
                    // dummy item with id set
                    PlayableItem::Track(FullTrack {
                        album: SimplifiedAlbum::default(),
                        artists: Vec::new(),
                        available_markets: Vec::new(),
                        disc_number: 0,
                        duration: Default::default(),
                        explicit: false,
                        external_ids: HashMap::new(),
                        external_urls: HashMap::new(),
                        href: None,
                        id: Some(track),
                        is_local: false,
                        is_playable: None,
                        linked_from: None,
                        restrictions: None,
                        name: String::new(),
                        popularity: 0,
                        preview_url: None,
                        track_number: 0,
                        r#type: Type::Track,
                    }),
                );
                wlf
            });

        if let Err(e) = QUEUE.write_cache(&(), with_last_fetched).await {
            warn!("Failed to add newly inserted queue item to disk cache: {e}");
        }

        Ok(())
    }
}

caching!(
    recently_played,
    Vec<PlayHistory>,
    // TODO: maybe use previous
    |_, _| async move {
        trace!("Getting queue");

        let spotify = spotify().await;
        Ok(retrying(
            move |_| async move { spotify.current_user_recently_played(None, None).await },
            (),
        )
        .await?
        .items)
    },
    RECENTLY_PLAYED,
    Duration::seconds(1)
);

#[cfg(feature = "server")]
fn simplified_playlist(playlist: &rspotify_model::FullPlaylist) -> SimplifiedPlaylist {
    let items = PlaylistTracksRef {
        href: playlist.items.href.clone(),
        total: playlist.items.total,
    };

    #[allow(deprecated)]
    SimplifiedPlaylist {
        collaborative: playlist.collaborative,
        external_urls: playlist.external_urls.clone(),
        href: playlist.href.clone(),
        id: playlist.id.clone(),
        images: playlist.images.clone(),
        name: playlist.name.clone(),
        owner: playlist.owner.clone(),
        public: playlist.public,
        snapshot_id: playlist.snapshot_id.clone(),
        tracks: items.clone(),
        items,
    }
}

caching!(
    user,
    PrivateUser,
    |_, _| async move {
        let spotify = spotify().await;
        retrying(move |_| async move { spotify.me().await }, ())
            .await
            .context("Failed to get current Spotify user")
    },
    USER,
    Duration::weeks(52)
);

/// Returns the [SimplifiedPlaylist] for the given `rating`, creating the playlist if it does not yet exist.
#[cfg(feature = "server")]
async fn get_or_create_playlist(rating: f32) -> Result<SimplifiedPlaylist> {
    let playlist_name = format!("{:.2}", rating);

    if let Some((_, playlist)) = rating_playlists_server()
        .await
        .value
        .iter()
        .find(|(playlist_rating, _)| *playlist_rating == rating)
    {
        return Ok(playlist.clone());
    }

    let spotify = spotify().await;
    let user = user_server().await;
    let playlist = retrying(
        move |(spotify, user_id, playlist_name)| async move {
            spotify
                .user_playlist_create(user_id.as_ref(), &playlist_name, Some(false), None, None)
                .await
        },
        (spotify, user.value.id.as_ref(), playlist_name),
    )
    .await
    .context("Failed to create rating playlist")?;
    let playlist = simplified_playlist(&playlist);

    let mut ret = RATING_PLAYLISTS
        .update_cache(&(), |playlists| {
            let mut playlists = playlists.cloned().unwrap_or_default();
            if !playlists
                .iter()
                .any(|(playlist_rating, _)| *playlist_rating == rating)
            {
                playlists.push((rating, playlist.clone()));
            }
            Some(playlists)
        })
        .await;

    if let Err(e) = ret {
        warn!("Failed to write newly created playlist to disk cache: {e}")
    }

    Ok(playlist)
}

/// Returns the [FullTrack] for the given `track_id`.
/// Checks ratings as cache first, falls back to fetching from the api.
/// Does not cache the api result.
#[cfg(feature = "server")]
async fn full_track_maybe_cached(
    track_id: &TrackId<'static>,
    ratings: &Analyzation,
) -> Result<FullTrack> {
    if let Some((track, _)) = ratings
        .tracks
        .iter()
        .find(|(track, _)| track.id.as_ref() == Some(track_id))
    {
        return Ok(track.clone());
    }

    let spotify = spotify().await;
    retrying(
        move |(spotify, track_id)| async move { spotify.track(track_id, None).await },
        (spotify, track_id.clone()),
    )
    .await
    .with_context(|| format!("Failed to fetch track {track_id}"))
    .map_err(Into::into)
}

#[cfg(feature = "server")]
async fn update_rating_caches(
    playlist: &SimplifiedPlaylist,
    track: &FullTrack,
    rating: f32,
) -> Result<f32> {
    use crate::spotify::{
        analyze::{DEFAULT_RATING, analyze},
        caching::WithLastFetched,
    };

    {
        use crate::spotify::caching::Cache;

        let res = RATING_PLAYLISTS
            .update_cache(&(), |playlists| {
                let mut playlists = playlists.cloned().unwrap_or_default();
                if !playlists
                    .iter()
                    .any(|(other_rating, _)| *other_rating == rating)
                {
                    playlists.push((rating, playlist.clone()));
                }

                Some(playlists)
            })
            .await;

        if let Err(e) = res {
            warn!("Failed to update rating playlists disk cache: {e}");
        }
    }

    let res = PLAYLIST_ITEMS
        .update_cache(&playlist.id, |items| {
            match items {
                Some(items) => {
                    let mut items = items.clone();
                    items.insert(0, track.clone());
                    Some(items)
                }
                None if playlist.items.total == 0 => Some(vec![track.clone()]),
                // playlist is not known to be empty, wait for next refetch for source of truth
                None => None,
            }
        })
        .await;
    if let Err(e) = res {
        warn!("Failed to update playlist items to disk cache: {e}")
    }

    let mut canonical_rating = DEFAULT_RATING; // should get overwritten by closure
    let ret = RATINGS
        .update_cache(&(), |cached_ratings| {
            let mut cached_ratings = cached_ratings.cloned().unwrap_or_default();

            let tracks = &mut cached_ratings.tracks;
            let entry = match tracks
                .iter_mut()
                .find(|(cached_track, _)| cached_track.id.as_ref() == track.id.as_ref())
            {
                Some((_, analyzation)) => analyzation,
                None => {
                    &mut tracks
                        .push_mut((track.clone(), TrackAnalyzation::default()))
                        .1
                }
            };
            entry.rating_history.push((UtcDateTime::now(), rating));

            cached_ratings = tokio::runtime::Handle::current().block_on(analyze(tracks.clone()));

            canonical_rating = cached_ratings.rating(
                track
                    .id
                    .as_ref()
                    .expect("full track used for rating updates should have an id")
                    .as_ref(),
            );

            Some(cached_ratings)
        })
        .await;
    if let Err(e) = ret {
        warn!("Failed to update ratings disk cache: {e}");
    }

    Ok(canonical_rating)
}

/// Returns the canonical rating, if there was a rating within the [RATING_OVERWRITE_WINDOW]
#[server]
pub async fn rating_if_recently_rated(track_id: TrackId<'static>) -> Result<Option<f32>> {
    crate::assert_authenticated!();
    let now = UtcDateTime::now();
    Ok(ratings_server()
        .await
        .value
        .tracks
        .iter()
        .find(|(track, _)| track.id.as_ref() == Some(&track_id))
        .and_then(|(_, analyzation)| {
            analyzation.rating_history.last().and_then(|(time, _)| {
                (now - *time <= RATING_OVERWRITE_WINDOW).then_some(analyzation.canonical_rating)
            })
        }))
}

#[server]
pub async fn add_rating(track_id: TrackId<'static>, rating: f32) -> Result<f32> {
    crate::assert_authenticated!();
    let rating = (rating * 100.0).round() / 100.0;
    let playlist = get_or_create_playlist(rating).await?;
    let cached_ratings = ratings_server().await;
    let track = full_track_maybe_cached(&track_id, &cached_ratings.value).await?;

    let spotify = spotify().await;
    retrying(
        move |(spotify, playlist_id, track_id)| async move {
            spotify
                .playlist_add_items(
                    playlist_id,
                    iter::once(PlayableId::Track(track_id.as_ref())),
                    Some(0),
                )
                .await
        },
        (spotify, playlist.id.clone(), track_id.clone()),
    )
    .await
    .with_context(|| {
        format!(
            "Failed to add track {track_id} to rating playlist {}",
            playlist.id
        )
    })?;
    update_rating_caches(&playlist, &track, rating).await
}

caching_hashmap!(
    full_artist,
    ArtistId<'static>,
    FullArtist,
    |artist_id, _| async move {
        info!("Getting artist with id {artist_id}");

        let spotify = spotify().await;

        retrying(
            move |artist_id| async move { spotify.artist(artist_id).await },
            artist_id.clone(),
        )
        .await
        .with_context(|| format!("Failed to get artist {artist_id}"))
    },
    ARTISTS,
    Duration::weeks(4) // assume artists are mostly static
);

pub async fn genres(artists: Vec<SimplifiedArtist>) -> HashSet<String> {
    let mut genres = HashSet::new();

    for artist in artists {
        if let Some(artist_id) = artist.id.map(ArtistId::into_static) {
            let full_artist = match full_artist(artist_id).await {
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
    |playlist_id, _| async move {
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
            match result.context("Failed to get playlist items")? {
                PlaylistItem {
                    item: Some(PlayableItem::Track(track)),
                    ..
                } => out.push(track),
                other => info!("Non-track playlist entry: {other:?}"),
            }
        }

        Ok(out)
    },
    PLAYLIST_ITEMS,
    Duration::HOUR
);

// (mis)use caching macro to periodically serialize and automatically deserialize
caching!(
    playback_options,
    PlaybackOptions,
    |_, previous| async move { Ok(previous.unwrap_or_default().value.clone()) },
    PLAYBACK_OPTIONS,
    Duration::seconds(5)
);

#[cfg(feature = "server")]
async fn update_playback_options(f: impl FnOnce(&mut PlaybackOptions)) -> Result<()> {
    crate::assert_authenticated!();

    Ok(PLAYBACK_OPTIONS
        .update_cache(&(), |options| {
            let mut options = options.cloned().unwrap_or_default();
            f(&mut options);
            Some(options)
        })
        .await
        .context("Failed to change playback options")?)
}

#[server]
pub async fn weighted_playback(playlist: PlaylistId<'static>, enabled: bool) -> Result<()> {
    update_playback_options(move |options| {
        if enabled {
            options.weighted_playback_playlists.insert(playlist);
        } else {
            options.weighted_playback_playlists.remove(&playlist);
        }
    })
    .await
}

#[server]
pub async fn playback_selection(selection: PlaybackSelection) -> Result<()> {
    update_playback_options(|options| options.selection = selection).await
}

#[server]
pub async fn playback_rating_cutoff(rating_cutoff: f32) -> Result<()> {
    update_playback_options(|options| options.rating_cutoff = rating_cutoff.clamp(0.0, 5.0)).await
}
