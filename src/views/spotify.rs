use std::iter;

use crate::ratings::{
    artist_genres, caching::use_server_fn, rating as fetch_rating, use_playback_state,
};
use dioxus::prelude::*;
use rspotify_model::{CurrentPlaybackContext, FullTrack, PlayableItem};
use time::Duration;

#[component]
pub fn Spotify() -> Element {
    let playback_state = use_playback_state();
    let charts = use_server_fn(charts, Duration::MINUTE);
    rsx!(
        Player { playback_state }
        h2 { "Charts" }
        match &*charts.read_unchecked() {
            Some(charts) => {
                rsx! {
                    for chart in charts {
                        iframe { width: 1920, height: 1080, srcdoc: "{chart}" }
                    }
                }
            }
            None => rsx! { "Loading charts..." },
        }
    )
}

#[component]
fn Player(playback_state: Signal<Option<Option<CurrentPlaybackContext>>>) -> Element {
    let track_id = playback_state.map(move |state| match state {
        Some(Some(CurrentPlaybackContext {
            item: Some(PlayableItem::Track(FullTrack { id, .. })),
            ..
        })) => id,
        _ => &None,
    });
    let rating = use_resource(move || {
        let id = track_id.read().clone();
        async move {
            match id {
                Some(id) => Some(fetch_rating(id).await),
                None => None,
            }
        }
    });

    let read = playback_state.read();
    let state = match *read {
        Some(ref state) => state.as_ref(),
        None => None,
    };
    let track = state
        .and_then(|state| state.item.as_ref())
        .and_then(|item| {
            if let PlayableItem::Track(track) = item {
                Some(track)
            } else {
                None
            }
        });
    let image = track.and_then(|track| track.album.images.first());
    rsx!(
        if let Some(image) = image {
            img {
                src: "{image.url}",
                width: image.width,
                height: image.height,
            }
            div {
                if let Some(track) = track {
                    h3 { "{track.name}" }
                }
                match &*rating.read() {
                    Some(Some(Ok(rating))) => format!("Rating: {rating}"),
                    Some(Some(Err(e))) => format!("Error getting rating: {e}"),
                    Some(None) => String::new(),
                    None => "Getting rating...".to_string(),
                }
            }
        }
    )
}

#[server]
async fn charts() -> Result<Vec<String>> {
    use crate::ratings::{
        artist_genres_server, average_rating_per_day, canonical_rating_correlations,
        canonical_rating_distribution, genre_proportions, num_ratings_history, ratings,
        ratings_server, song_canonical_rating_histories,
    };

    use charming::HtmlRenderer;

    let renderer = HtmlRenderer::new("Renderer", 1920, 1080);

    let analyzation = ratings_server().await;
    let artist_genres = artist_genres_server().await;

    Ok([
        canonical_rating_distribution,
        average_rating_per_day,
        num_ratings_history,
        song_canonical_rating_histories,
        canonical_rating_correlations,
    ]
    .into_iter()
    .map(|f| f(&analyzation))
    .chain([genre_proportions].map(|f| f(&analyzation, &artist_genres)))
    .map(|chart| {
        renderer
            .render(&chart)
            .expect("Rendering chart shouldn't fail")
    })
    .collect())
}
