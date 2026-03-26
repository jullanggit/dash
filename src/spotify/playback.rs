use crate::spotify::spotify;
use dioxus::prelude::*;
use rspotify_model::{Context, CurrentPlaybackContext, PlaylistId, TrackId, Type};
use std::time::Duration;

#[cfg(feature = "server")]
pub async fn handle_weighted_playback() -> ! {
    loop {
        queue_random_song().await;

        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

// TODO:
// - only add a song to queue if it is empty
// - store user settings for which playlists should be affected
#[cfg(feature = "server")]
async fn queue_random_song() {
    use crate::spotify::weighted_playback_enabled_server;
    use crate::spotify::{
        playback_state_server, playlist_tracks_server, queue_server, retrying, saved_tracks_server,
    };
    use rspotify::prelude::OAuthClient;

    let spotify = spotify().await;

    // only queue a song if the queue is empty
    let queue = queue_server().await;
    if !queue.is_empty() {
        return;
    }

    let context = playback_state_server().await;

    if let Some(CurrentPlaybackContext {
        context: Some(Context { _type, uri, .. }),
        ..
    }) = context
    {
        let tracks = match _type {
            Type::Playlist => {
                let id =
                    PlaylistId::from_id(uri).expect("_type = playlist uri should be a playlist id");
                if weighted_playback_enabled_server().await.contains(&id) {
                    Some(
                        playlist_tracks_server(id)
                            .await
                            .iter()
                            .filter_map(|track| track.id.clone())
                            .collect::<Vec<_>>(),
                    )
                } else {
                    None
                }
            }
            Type::Collection => Some(saved_tracks_server().await.into_iter().collect::<Vec<_>>()),
            _ => None,
        };

        if let Some(tracks) = tracks {
            let track = choose_random_song(&tracks).await;

            if let Some(track) = track {
                let res = retrying(
                    move |(spotify, track)| async {
                        spotify.add_item_to_queue(track.into(), None).await
                    },
                    (spotify, track.clone()),
                )
                .await;
                if let Err(e) = res {
                    warn!("Failed to add track {track} to queue: {e}")
                }
            }
        }
    }
}

#[cfg(feature = "server")]
async fn choose_random_song<'a>(tracks: &'a [TrackId<'_>]) -> Option<TrackId<'a>> {
    use crate::spotify::ratings_server;
    use rand::{RngExt, rng};

    let ratings = ratings_server().await;
    let weights = tracks
        .iter()
        .map(|track| weight(ratings.rating(track.as_ref())));

    let total_weight: f32 = weights.clone().sum();

    let mut value = rng().random::<f32>() * total_weight;
    tracks
        .iter()
        .zip(weights)
        .find(|(_, weight)| {
            value -= weight;
            value <= 0.
        })
        .map(|(track, _)| track.as_ref())
}

// TODO: add more parameters, i.e. recently played songs, playlist membership etc.
fn weight(rating: f32) -> f32 {
    2f32.powf(rating)
}
