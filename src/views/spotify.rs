use crate::ratings::{rating, use_playback_state};
use dioxus::prelude::*;
use rspotify_model::{CurrentPlaybackContext, PlayableItem};

#[component]
pub fn Spotify() -> Element {
    let charts = use_server_future(move || async move { charts().await })?;
    let playback_state = use_playback_state();
    rsx!(
        Player { playback_state }
        h2 { "Charts" }
        match &*charts.read_unchecked() {
            Some(Ok(charts)) => {
                rsx! {
                    for chart in charts {
                        iframe { width: 1920, height: 1080, srcdoc: "{chart}" }
                    }
                }
            }
            Some(Err(e)) => rsx! { "Error loading charts: {e}" },
            None => rsx! { "Loading charts..." },
        }
    )
}

#[component]
fn Player(playback_state: Signal<Option<Option<CurrentPlaybackContext>>>) -> Element {
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
    let rating = track.and_then(|track| track.id.clone()).map(|id| {
        use_resource(move || {
            let id = id.clone();
            async move { rating(id).await }
        })
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
                match rating {
                    Some(rating) => {
                        match &*rating.read() {
                            Some(Ok(rating)) => format!("Rating: {rating}"),
                            Some(Err(e)) => format!("Error getting rating: {e}"),
                            None => "Getting rating...".to_string(),
                        }
                    }
                    None => String::new(),
                }
            }
        }
    )
}

#[server]
async fn charts() -> Result<[String; 5]> {
    use crate::ratings::{
        average_rating_per_day, canonical_rating_correlations, canonical_rating_distribution,
        num_ratings_history, ratings, song_canonical_rating_histories,
    };

    use charming::HtmlRenderer;

    let renderer = HtmlRenderer::new("Renderer", 1920, 1080);

    let analyzation = ratings()
        .await
        .expect("Never errors on server-to-server calls");

    Ok([
        canonical_rating_distribution,
        average_rating_per_day,
        num_ratings_history,
        song_canonical_rating_histories,
        canonical_rating_correlations,
    ]
    .map(|f| {
        let chart = f(&analyzation);
        renderer
            .render(&chart)
            .expect("Rendering chart shouldn't fail")
    }))
}
