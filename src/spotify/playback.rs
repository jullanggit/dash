use dioxus::prelude::*;
use rspotify::prelude::OAuthClient;
use rspotify_model::{CurrentPlaybackContext, PlaylistId, TrackId, Type};

use crate::spotify::{playlist_tracks_server, spotify};

// TODO:
// - only add a song to queue if it is empty
// - store user settings for which playlists should be affected
#[server]
async fn queue_random_song(context: CurrentPlaybackContext) -> Result<()> {
    let spotify = spotify().await;

    if let Some(context) = context.context {
        match context._type {
            Type::Artist => todo!(),
            Type::Album => todo!(),
            Type::Track => todo!(),
            Type::Playlist => {
                let tracks = playlist_tracks_server(
                    PlaylistId::from_id(context.uri)
                        .expect("_type = playlist uri should be a playlist id"),
                )
                .await
                .iter()
                .filter_map(|track| track.id.clone())
                .collect::<Vec<_>>();

                let track = choose_random_song(&tracks).await;

                if let Some(track) = track {
                    spotify.add_item_to_queue(track.into(), None);
                }
            }
            Type::User => todo!(),
            Type::Show => todo!(),
            Type::Episode => todo!(),
            Type::Collection => todo!(),
            Type::Collectionyourepisodes => todo!(),
            Type::Unknown(_) => todo!(),
        }
    }

    Ok(())
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
