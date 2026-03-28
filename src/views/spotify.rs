use crate::spotify::{
    add_rating, caching::use_server_fn, genres, rating as fetch_rating, use_playback_state,
};
use dioxus::prelude::*;
use rspotify_model::{CurrentPlaybackContext, FullTrack, PlayableItem, TrackId};
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
    let mut rating = use_resource(move || {
        let id = track_id.read().clone();
        async move {
            match id {
                Some(id) => Some(fetch_rating(id).await),
                None => None,
            }
        }
    });
    let mut pending_rating = use_signal(|| None::<(TrackId<'static>, f32)>);

    let genres = use_resource(move || async move {
        // let to avoid holding the 'read' across await points
        let Some(Some(CurrentPlaybackContext {
            item: Some(PlayableItem::Track(FullTrack { artists, .. })),
            ..
        })) = playback_state.read().clone()
        else {
            return None;
        };
        let mut genres = genres(artists.clone())
            .await
            .into_iter()
            .collect::<Vec<_>>();
        genres.sort();
        Some(genres)
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
    let current_track_id = track_id.read().clone();
    let pending_rating_value =
        pending_rating
            .read()
            .as_ref()
            .and_then(|(pending_track_id, pending_rating)| {
                (Some(pending_track_id) == current_track_id.as_ref()).then_some(*pending_rating)
            });
    let fetched_rating = match &*rating.read() {
        Some(Some(Ok(rating))) => Some(*rating),
        _ => None,
    };
    let current_rating = pending_rating_value.or(fetched_rating);
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
                match &*genres.read() {
                    Some(Some(genres)) if !genres.is_empty() => {
                        format!(
                            "Genres: {}",
                            genres.iter().cloned().intersperse(", ".into()).collect::<String>(),
                        )
                    }
                    Some(_) => String::new(),
                    None => "Getting genres...".to_string(),
                }
                br {}
                br {}
                HoverSlider {
                    current_rating,
                    on_select: move |selected_rating| async move {
                        let Some(track_id) = track_id.read().clone() else {
                            return;
                        };
                        match add_rating(track_id.clone(), selected_rating as f32).await {
                            Ok(canonical_rating) => {
                                pending_rating.set(Some((track_id, canonical_rating)));
                                rating.restart();
                            }
                            Err(error) => {
                                pending_rating.set(None);
                                error!("Failed to submit rating: {error}");
                            }
                        }
                    },
                }
            }
        }
    )
}

#[component]
fn HoverSlider(current_rating: Option<f32>, on_select: EventHandler<f64>) -> Element {
    let mut hovered = use_signal(|| false);
    let mut cursor_x = use_signal(|| 0.0_f64);
    let mut width = use_signal(|| 1.0_f64); // avoid divide-by-zero
    let resting_progress = current_rating.unwrap_or(0.0).clamp(0.0, 5.0) as f64;

    rsx! {
        div {
            style: "
                position: relative;
                width: 400px;
                height: 48px;
                margin: 0 auto;
                background: #222;
                border-radius: 8px;
                overflow: hidden;
                cursor: pointer;
                user-select: none;
            ",

            onmounted: move |evt| {
                spawn(async move {
                    if let Ok(rect) = evt.data().get_client_rect().await {
                        width.set(rect.size.width);
                    }
                });
            },

            onmouseenter: move |_| {
                hovered.set(true);
            },

            onmouseleave: move |_| {
                hovered.set(false);
            },

            onmousemove: move |evt| {
                let x = evt.element_coordinates().x;
                let x = x.clamp(0.0, *width.read());
                cursor_x.set(x);
            },

            onclick: move |evt| {
                let x = evt.element_coordinates().x.clamp(0.0, *width.read());
                let rating = ((x / *width.read()).clamp(0.0, 1.0)) * 5.0;
                on_select.call(rating);
            },

            div { style: "
                    position: absolute;
                    inset: 0;
                    display: flex;
                    align-items: center;
                    justify-content: center;
                    color: white;
                    font-weight: 600;
                    pointer-events: none;
                ",
                if *hovered.read() {
                    {format!("{:.2}", ((*cursor_x.read() / *width.read()).clamp(0.0, 1.0)) * 5.0)}
                } else {
                    {format!("{resting_progress:.2}")}
                }
            }

            div {
                style: format!(
                    "
                                                                                                                        position: absolute;
                                                                                                                        top: 0;
                                                                                                                        bottom: 0;
                                                                                                                        left: {}px;
                                                                                                                        width: 2px;
                                                                                                                        background: white;
                                                                                                                        transform: translateX(-50%);
                                                                                                                        pointer-events: none;
                                                                                                                    ",
                    if *hovered.read() {
                        *cursor_x.read()
                    } else {
                        (resting_progress / 5.0) * *width.read()
                    },
                ),
            }
        }
    }
}

#[server]
async fn charts() -> Result<Vec<String>> {
    use crate::spotify::{
        artist_proportions, average_rating_per_day, canonical_rating_correlations,
        canonical_rating_distribution, genre_proportions, num_ratings_history, ratings_server,
        song_canonical_rating_histories,
    };

    use charming::HtmlRenderer;

    let renderer = HtmlRenderer::new("Renderer", 1920, 1080);

    let analyzation = ratings_server().await;

    Ok([
        canonical_rating_distribution,
        average_rating_per_day,
        num_ratings_history,
        song_canonical_rating_histories,
        canonical_rating_correlations,
        genre_proportions,
        artist_proportions,
    ]
    .into_iter()
    .map(|f| f(&analyzation))
    .map(|chart| {
        renderer
            .render(&chart)
            .expect("Rendering chart shouldn't fail")
    })
    .collect())
}
