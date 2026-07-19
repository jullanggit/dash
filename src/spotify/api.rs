#[cfg(feature = "server")]
use crate::spotify::caching::Cache;
use crate::{
    caching, caching_hashmap,
    spotify::{
        analyze::{Analyzation, RATING_OVERWRITE_WINDOW, TrackAnalyzation, TrackKey},
        caching::use_server_fn,
        playback::{PlaybackOptions, PlaybackSelection},
    },
};
use dioxus::{fullstack::reqwest, prelude::*};
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
use rspotify_http::BaseHttpClient;
#[cfg(feature = "server")]
use rspotify_model::Page;
use rspotify_model::{
    ArtistId, CurrentPlaybackContext, FullArtist, FullTrack, PlayHistory, PlayableId, PlayableItem,
    PlaylistId, PlaylistItem, PlaylistTracksRef, PrivateUser, SimplifiedPlaylist, TrackId,
};
#[cfg(feature = "server")]
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fmt::{self, Display, Formatter},
    iter,
    sync::{LazyLock, OnceLock},
};
#[cfg(feature = "server")]
use std::{fmt::Debug, pin::Pin};
use time::{Duration, UtcDateTime};
#[cfg(feature = "server")]
use tokio::time::sleep;

#[cfg(feature = "server")]
static SPOTIFY: OnceLock<AuthCodeSpotify> = OnceLock::new();

macro_rules! RequestPermits {
    ($($path:ident),*) => {
        #[cfg(feature = "server")]
        use tokio::sync::{Semaphore, SemaphorePermit};

        $(
            #[cfg(feature = "server")]
            #[allow(non_snake_case)]
            #[allow(non_upper_case_globals)]
            static ${concat(_, $path, _request_permit)}: Semaphore = Semaphore::const_new(1);
        )*

        #[cfg(feature = "server")]
        #[derive(Debug, Clone, Copy)]
        enum RequestPermit {
            $($path),*
        }
        #[cfg(feature = "server")]
        impl RequestPermit {
            /// Acquire the permit, wrapping the error in a spicetify-compatible type
            async fn acquire(&self) -> ClientResult<SemaphorePermit<'static>> {
                match self {
                    $(
                        Self::$path => ${concat(_, $path, _request_permit)}.acquire().await.map_err(|e| ClientError::Io(std::io::Error::other(e)))
                    ),*
                }
            }
        }
    };
}

RequestPermits!(
    // /me/playlists
    MyPlaylists,
    // /playlists/*
    Playlists,
    // /me/tracks
    SavedTracks,
    // /me/player
    Player,
    // /track
    Tracks,
    // /artists
    Artists,
    // /me
    Me,
    // last.fm track.getTopTags
    LastFmTopTags
);

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
                    redirect_uri: "http://127.0.0.1:8888".into(),
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
    Vec<(f32, Vec<SimplifiedPlaylist>)>,
    |_, _| async move {
        let spotify = spotify().await;
        let mut groups: Vec<(f32, Vec<SimplifiedPlaylist>)> = Vec::new();

        trace!("Getting rating playlists");

        let mut response = paginate_retrying(
            move |offset| {
                let spotify = spotify.clone();
                async move {
                    trace!("[SPOTIFY API LOG] current user playlists, offset {offset}");
                    spotify
                        .current_user_playlists_manual(None, Some(offset))
                        .await
                }
            },
            RequestPermit::MyPlaylists,
        )
        .await;

        while let Some(result) = response.next().await {
            let playlist = result.context("Error getting playlist")?;
            if let Ok(rating) = playlist.name.parse::<f32>()
                && (0.0..=5.0).contains(&rating)
            {
                match groups.iter_mut().find(|(r, _)| *r == rating) {
                    Some((_, group)) => group.push(playlist.clone()),
                    None => groups.push((rating, vec![playlist.clone()])),
                }
            }
        }

        // sort by ascending number of tracks to balance playlist filling
        for (_, group) in &mut groups {
            group.sort_unstable_by_key(|p| p.items.total);
        }
        groups.sort_unstable_by(|(a, _), (b, _)| a.partial_cmp(b).unwrap());

        Ok(groups)
    },
    RATING_PLAYLISTS,
    Duration::minutes(3)
);

/// Paginates the given function, retrying any too-many-request errors.
/// Returns early if any other errors are encountered.
#[cfg(feature = "server")]
async fn paginate_retrying<F, Fut, T>(
    f: F,
    permit: RequestPermit,
) -> Pin<Box<impl Stream<Item = ClientResult<T>>>>
where
    F: Fn(u32) -> Fut,
    Fut: Future<Output = ClientResult<Page<T>>>,
    T: DeserializeOwned + Debug,
{
    let mut offset = 0;
    Box::pin(async_stream::stream! {
        loop {
            let page = retrying(&f, offset, permit).await?;

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
pub async fn retrying<F, Args, Fut, T>(f: F, args: Args, permit: RequestPermit) -> ClientResult<T>
where
    F: Fn(Args) -> Fut,
    Args: Clone,
    Fut: Future<Output = ClientResult<T>>,
    T: DeserializeOwned + Debug,
{
    let mut num_tries = 0;
    let permit = permit.acquire().await?;
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

caching!(
    ratings,
    Analyzation,
    // get ratings. Only re-fetch ratings within the last 15 minutes.
    |_, previous| async move {
        use crate::spotify::analyze::analyze;
        use std::collections::HashMap;

        let spotify = spotify().await.clone();
        let playlists = &rating_playlists_server().await.value;
        let previous = &previous.unwrap_or_default().value;
        let previous_snapshot_ids = previous.playlist_snapshot_ids.clone();
        let mut ratings = previous.tracks.clone();

        trace!("Getting ratings");

        use crate::spotify::analyze::TrackKey;

        let mut current_snapshot_ids = playlists
            .iter()
            .flat_map(|(_, playlists)| {
                playlists
                    .iter()
                    .map(|playlist| (playlist.id.clone(), playlist.snapshot_id.clone()))
            })
            .collect::<HashMap<_, _>>();

        let mut last_full_refetch = previous.last_full_refetch.clone();
        let oldest_threshold = UtcDateTime::now() - Duration::days(1);

        // most overdue playlist
        let overdue_id: Option<PlaylistId<'static>> = playlists
            .iter()
            .flat_map(|(_, playlists)| playlists.iter())
            .filter_map(|playlist| {
                let last = last_full_refetch
                    .get(&playlist.id)
                    .copied()
                    .unwrap_or(UtcDateTime::MIN);
                (last < oldest_threshold).then_some((last, playlist.id.clone()))
            })
            .min_by_key(|(last, _)| *last)
            .map(|(_, id)| id);

        for (rating, playlists) in playlists.iter() {
            for playlist in playlists.iter() {
                let is_overdue_playlist = overdue_id.as_ref() == Some(&playlist.id);

                if !is_overdue_playlist
                    && previous_snapshot_ids
                        .get(&playlist.id)
                        .is_some_and(|previous| *previous == playlist.snapshot_id)
                {
                    continue;
                }

                let spotify_clone = spotify.clone();
                let mut items = paginate_retrying(
                    move |offset| {
                        let spotify = spotify_clone.clone();
                        let id = playlist.id.clone();
                        async move {
                            trace!("[SPOTIFY API LOG] playlist items, id {id}, offset {offset}");
                            spotify
                                .playlist_items_manual(id, None, None, None, Some(offset))
                                .await
                        }
                    },
                    RequestPermit::Playlists,
                )
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
                            let key = TrackKey::from_track(&item);
                            let entry = ratings
                                .entry(key)
                                .or_insert_with(|| (item, TrackAnalyzation::default()));

                            let data = (
                                UtcDateTime::from_unix_timestamp(added_at.timestamp()).unwrap(),
                                *rating,
                            );

                            if entry.1.rating_history.contains(&data) {
                                // not overdue => don't fully refetch, assume old data hasn't changed
                                if !is_overdue_playlist {
                                    break;
                                }
                            } else {
                                entry.1.rating_history.push(data);
                            }
                        }
                        other => {
                            return Err(anyhow::anyhow!(
                                "Unexpected format for rating playlist entry: {other:?}"
                            ));
                        }
                    }
                }

                if is_overdue_playlist {
                    last_full_refetch.insert(playlist.id.clone(), UtcDateTime::now());
                }
            }
        }

        let mut analyzation = analyze(ratings).await;
        analyzation.playlist_snapshot_ids = current_snapshot_ids;
        analyzation.last_full_refetch = last_full_refetch;
        Ok(analyzation)
    },
    RATINGS,
    Duration::minutes(3)
);

caching!(
    saved_tracks,
    HashMap<TrackKey, TrackId<'static>>,
    // get ratings. Only re-fetch ratings within the last 15 minutes.
    |_, previous| async move {
        use crate::spotify::analyze::TrackKey;

        let spotify = spotify().await;
        let mut saved_tracks = previous
            .map(|previous| previous.value.clone())
            .unwrap_or_default();

        trace!("Getting saved tracks");

        let mut items = paginate_retrying(
            move |offset| {
                let spotify = spotify.clone();
                async move {
                    trace!("[SPOTIFY API LOG] saved_tracks, offset {offset}");
                    spotify
                        .current_user_saved_tracks_manual(None, None, Some(offset))
                        .await
                }
            },
            RequestPermit::SavedTracks,
        )
        .await;

        // assumptions:
        // The first initialization fetches all available items.
        // The saved tracks is only ever appended to. TODO: refetch the entire playlist from time to time
        while let Some(result) = items.next().await {
            let track = &result.context("Failed to get saved track")?.track;
            if let Some(track_id) = track.id.clone() {
                let key = TrackKey::from_track(track);
                if saved_tracks.insert(key, track_id).is_some() {
                    break; // already had this key, assume we've seen all previously saved tracks
                }
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
            RequestPermit::Player,
        )
        .await?)
    },
    PLAYBACK_STATE,
    Duration::seconds(2)
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
            RequestPermit::Player,
        )
        .await?
        .queue)
    },
    QUEUE,
    Duration::seconds(2)
);
#[cfg(feature = "server")]
pub async fn add_to_queue(
    track_key: TrackKey,
    track_id: TrackId<'static>,
) -> Result<(), anyhow::Error> {
    use rspotify_model::{SimplifiedAlbum, SimplifiedArtist, Type};
    use std::collections::HashMap;

    let spotify = spotify().await;
    let res =
        retrying(
            move |(spotify, track_id)| async move {
                spotify.add_item_to_queue(track_id.into(), None).await
            },
            (spotify, track_id.clone()),
            RequestPermit::Player,
        )
        .await;
    if let Err(e) = res {
        Err(anyhow::anyhow!(
            "Failed to add track {track_id} to queue: {e}"
        ))
    } else {
        let res = QUEUE
            .update_cache(&(), |queue| {
                let mut queue = queue.cloned().unwrap_or_default();
                queue.insert(
                    0,
                    PlayableItem::Track(FullTrack {
                        album: SimplifiedAlbum::default(),
                        artists: track_key
                            .artists
                            .iter()
                            .map(|name| SimplifiedArtist {
                                name: name.clone(),
                                external_urls: HashMap::new(),
                                href: None,
                                id: None,
                            })
                            .collect(),
                        available_markets: Vec::new(),
                        disc_number: 0,
                        duration: Default::default(),
                        explicit: false,
                        external_ids: HashMap::new(),
                        external_urls: HashMap::new(),
                        href: None,
                        id: Some(track_id),
                        is_local: false,
                        is_playable: None,
                        linked_from: None,
                        restrictions: None,
                        name: track_key.name.clone(),
                        popularity: 0,
                        preview_url: None,
                        track_number: 0,
                        r#type: Type::Track,
                    }),
                );
                Some(queue)
            })
            .await;

        if let Err(e) = res {
            warn!("Failed to add newly inserted queue item to disk cache: {e}");
        }

        Ok(())
    }
}

caching!(
    recently_played,
    Vec<PlayHistory>,
    // TODO: also consider currently playing song as recently played
    |_, _| async move {
        trace!("Getting queue");

        let spotify = spotify().await;
        Ok(retrying(
            move |_| async move { spotify.current_user_recently_played(None, None).await },
            (),
            RequestPermit::Player,
        )
        .await?
        .items)
    },
    RECENTLY_PLAYED,
    Duration::seconds(3)
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
        retrying(
            move |_| async move { spotify.me().await },
            (),
            RequestPermit::Me,
        )
        .await
        .context("Failed to get current Spotify user")
    },
    USER,
    Duration::weeks(52)
);

/// Returns the [SimplifiedPlaylist] for the given `rating`, creating the playlist if it does not yet exist.
/// If multiple playlists exist for the same rating, returns the one with the fewest tracks.
#[cfg(feature = "server")]
async fn get_or_create_playlist(rating: f32) -> Result<SimplifiedPlaylist> {
    let playlist_name = format!("{:.2}", rating);

    if let Some((_, playlists)) = rating_playlists_server()
        .await
        .value
        .iter()
        .find(|(playlist_rating, _)| *playlist_rating == rating)
    {
        return Ok(playlists[0].clone());
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
        RequestPermit::MyPlaylists,
    )
    .await
    .context("Failed to create rating playlist")?;
    let playlist = simplified_playlist(&playlist);

    let ret = RATING_PLAYLISTS
        .update_cache(&(), |playlists| {
            let mut playlists = playlists.cloned().unwrap_or_default();
            match playlists.iter_mut().find(|(r, _)| *r == rating) {
                Some((_, group)) => {
                    if !group.iter().any(|p| p.id == playlist.id) {
                        group.push(playlist.clone());
                        group.sort_unstable_by_key(|p| p.items.total);
                    }
                    Some(playlists)
                }
                None => {
                    playlists.push((rating, vec![playlist.clone()]));
                    Some(playlists)
                }
            }
        })
        .await;

    if let Err(e) = ret {
        warn!("Failed to write newly created playlist to disk cache: {e}")
    }

    Ok(playlist)
}

/// Returns the [FullTrack] for the given `track_key`.
/// Checks ratings as cache first, falls back to fetching from the api using `track_id`.
/// Does not cache the api result.
#[cfg(feature = "server")]
async fn full_track_maybe_cached(
    track_key: &TrackKey,
    track_id: &TrackId<'static>,
    ratings: &Analyzation,
) -> Result<FullTrack> {
    if let Some((track, _)) = ratings.tracks.get(track_key) {
        return Ok(track.clone());
    }

    let spotify = spotify().await;
    retrying(
        move |(spotify, track_id)| async move { spotify.track(track_id, None).await },
        (spotify, track_id.clone()),
        RequestPermit::Tracks,
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
    use std::sync::Arc;

    use crate::spotify::analyze::analyze;

    {
        use crate::spotify::caching::Cache;

        let res = RATING_PLAYLISTS
            .update_cache(&(), |playlists| {
                let mut playlists = playlists.cloned().unwrap_or_default();
                match playlists.iter_mut().find(|(r, _)| *r == rating) {
                    Some((_, group)) => {
                        if !group.iter().any(|p| p.id == playlist.id) {
                            group.push(playlist.clone());
                            group.sort_unstable_by_key(|p| p.items.total);
                        }
                    }
                    None => {
                        playlists.push((rating, vec![playlist.clone()]));
                    }
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

    use crate::spotify::analyze::TrackKey;

    let mut cached_ratings =
        Arc::unwrap_or_clone(RATINGS.read_cache(&()).await.unwrap_or_default());

    let track_key = TrackKey::from_track(track);
    let entry = cached_ratings
        .value
        .tracks
        .entry(track_key)
        .or_insert_with(|| (track.clone(), TrackAnalyzation::default()));
    entry.1.rating_history.push((UtcDateTime::now(), rating));

    cached_ratings.value = analyze(cached_ratings.value.tracks.clone()).await;

    let canonical_rating = cached_ratings.value.rating(&TrackKey::from_track(track));

    if let Err(e) = RATINGS.write_cache(&(), Arc::new(cached_ratings)).await {
        warn!("Failed to update ratings disk cache: {e}");
    }

    Ok(canonical_rating)
}

/// Returns the canonical rating, if there was a rating within the [RATING_OVERWRITE_WINDOW]
#[server]
pub async fn rating_if_recently_rated(track_key: TrackKey) -> Result<Option<f32>> {
    crate::assert_authenticated!();
    let now = UtcDateTime::now();
    Ok(ratings_server()
        .await
        .value
        .tracks
        .get(&track_key)
        .and_then(|(_, analyzation)| {
            analyzation.rating_history.last().and_then(|(time, _)| {
                (now - *time <= RATING_OVERWRITE_WINDOW).then_some(analyzation.canonical_rating)
            })
        }))
}

#[server]
pub async fn add_rating(
    track_key: TrackKey,
    track_id: TrackId<'static>,
    rating: f32,
) -> Result<f32> {
    crate::assert_authenticated!();
    let rating = (rating * 100.0).round() / 100.0;
    let playlist = get_or_create_playlist(rating).await?;
    let cached_ratings = ratings_server().await;
    let track = full_track_maybe_cached(&track_key, &track_id, &cached_ratings.value).await?;

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
        RequestPermit::Playlists,
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
            RequestPermit::Artists,
        )
        .await
        .with_context(|| format!("Failed to get artist {artist_id}"))
    },
    ARTISTS,
    Duration::weeks(4) // assume artists are mostly static
);

pub async fn genres(track: &FullTrack) -> HashSet<String> {
    let mut genres = HashSet::new();

    fn add_genres(genres: &mut HashSet<String>, new_genres: impl IntoIterator<Item = String>) {
        for genre in new_genres {
            genres.insert(genre.to_lowercase());
        }
    }

    for (i, artist) in track.artists.iter().enumerate() {
        if let Some(artist_id) = artist.id.clone().map(ArtistId::into_static) {
            #[cfg(feature = "server")]
            let full_artist = full_artist_server(artist_id).await.value.clone();
            #[cfg(not(feature = "server"))]
            let full_artist = match full_artist(artist_id).await {
                Ok(artist) => artist,
                Err(e) => {
                    error!("Failed to fetch artist {}: {e}", artist.name);
                    continue;
                }
            };
            add_genres(&mut genres, full_artist.genres);
        }

        #[cfg(feature = "server")]
        let lastfm_artist_genres = lastfm_artist_top_tags_server(artist.name.clone())
            .await
            .value
            .clone();
        #[cfg(not(feature = "server"))]
        let lastfm_artist_genres = match lastfm_artist_top_tags(artist.name.clone()).await {
            Ok(genres) => genres,
            Err(e) => {
                error!("Failed to fetch last.fm artist genres: {e}");
                Vec::new()
            }
        };
        add_genres(
            &mut genres,
            lastfm_artist_genres.into_iter().map(|tag| tag.name),
        );

        // only fetch track genres for the first artist
        if i == 0 {
            let key = LastFmTopTagsKey {
                track: track.name.clone(),
                artist: artist.name.clone(),
            };
            #[cfg(feature = "server")]
            let lastfm_genres = lastfm_top_tags_server(key).await.value.clone();
            #[cfg(not(feature = "server"))]
            let lastfm_genres = match lastfm_top_tags(key).await {
                Ok(genres) => genres,
                Err(e) => {
                    error!("Failed to fetch last.fm genres: {e}");
                    Vec::new()
                }
            };
            add_genres(&mut genres, lastfm_genres.into_iter().map(|tag| tag.name));
        }

        // cleanup
        genres.remove(&artist.name.to_lowercase());
    }

    // cleanup
    genres.remove(&track.name.to_lowercase());

    genres
}

caching_hashmap!(
    playlist_tracks,
    PlaylistId<'static>,
    Vec<FullTrack>,
    |playlist_id, _| async move {
        let spotify = spotify().await;

        let mut out = Vec::new();
        let mut items = paginate_retrying(
            move |offset| {
                let spotify = spotify.clone();
                let id = playlist_id.clone();
                async move {
                    trace!("[SPOTIFY API LOG] playlist items, id {id}, offset {offset}");
                    spotify
                        .playlist_items_manual(id, None, None, None, Some(offset))
                        .await
                }
            },
            RequestPermit::Playlists,
        )
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

structstruck::strike!(
    #[structstruck::each[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]]
    pub struct LastFmTopTagsResponse {
        toptags: Option<pub struct LastFmTopTags {
            tag: Vec<struct LastFmTag {
                name: String,
                count: u32,
            }>,
        }>,
        error: Option<u8>,
        message: Option<String>,
    }
);

static LASTFM_API_KEY: LazyLock<String> = LazyLock::new(|| {
    let dotenv = std::fs::read_to_string(".env").expect("Please set LASTFM_API_KEY in .env");
    dotenv
        .lines()
        .find_map(|line| line.strip_prefix("LASTFM_API_KEY="))
        .expect("Please set LASTFM_API_KEY in .env")
        .to_string()
});

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, Hash)]
struct LastFmTopTagsKey {
    track: String,
    artist: String,
}
impl Display for LastFmTopTagsKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{} - {}", self.track, self.artist)
    }
}

#[cfg(feature = "server")]
async fn lastfm_top_tags_inner(
    key: LastFmTopTagsKey,
    method: &str,
) -> anyhow::Result<Vec<LastFmTag>> {
    info!("Getting last.fm top tags for {key}");

    let client = reqwest::Client::new();

    retrying(
        move |(key, client)| async move {
            let response = client
                .get("https://ws.audioscrobbler.com/2.0/")
                .query(&[
                    ("method", method),
                    ("artist", &key.artist),
                    ("track", &key.track),
                    ("api_key", &LASTFM_API_KEY),
                    ("format", "json"),
                    ("autocorrect", "1"),
                ])
                .send()
                .await
                .map_err(|err| rspotify_http::HttpError::Client(err))?;
            if !response.status().is_success() {
                return Err(ClientError::from(rspotify_http::HttpError::StatusCode(
                    response,
                )));
            }

            let deserialized = response
                .json::<LastFmTopTagsResponse>()
                .await
                .map_err(|err| rspotify_http::HttpError::Client(err))?;

            match deserialized {
                LastFmTopTagsResponse {
                    toptags: Some(toptags),
                    ..
                } => Ok(toptags.tag),
                LastFmTopTagsResponse { error: Some(6), .. } => Ok(Vec::new()), // not found
                LastFmTopTagsResponse { error, message, .. } => Err(ClientError::Io(std::io::Error::new(std::io::ErrorKind::Other,
                    format!("Failed to get last.fm top tags for {key}: error={error:?} message={message:?}")))),
            }
        },
        (key.clone(), client),
        RequestPermit::LastFmTopTags,
    )
    .await
    .with_context(|| format!("Failed to get last.fm top tags for {key}"))
}

caching_hashmap!(
    lastfm_top_tags,
    LastFmTopTagsKey,
    Vec<LastFmTag>,
    |key, _| lastfm_top_tags_inner(key, "track.getTopTags"),
    LASTFM_TOP_TAGS,
    Duration::weeks(4) // assume artists are mostly static
);

caching_hashmap!(
    lastfm_artist_top_tags,
    String,
    Vec<LastFmTag>,
    |key, _| lastfm_top_tags_inner(
        LastFmTopTagsKey {
            artist: key,
            track: String::new()
        },
        "artist.getTopTags"
    ),
    LASTFM_ARTIST_TOP_TAGS,
    Duration::weeks(4) // assume artists are mostly static
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
