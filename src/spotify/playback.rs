#[cfg(feature = "server")]
use rspotify_model::TrackId;

use crate::spotify::ratings;

#[cfg(feature = "server")]
async fn choose_random_song<'a>(tracks: &'a [TrackId<'_>]) -> Option<TrackId<'a>> {
    use rand::{RngExt, rng};

    use crate::spotify::ratings_server;

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
