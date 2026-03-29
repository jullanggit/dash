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
                        iframe {
                            width: 1920,
                            height: 1080,
                            srcdoc: "{chart}",
                            scrolling: "no",
                            style: "border: 0; overflow: hidden;",
                        }
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
    let mut canonical_history_chart = use_resource(move || {
        let id = track_id.read().clone().map(TrackId::into_static);
        async move {
            match id {
                Some(id) => Some(canonical_rating_history_chart(id).await),
                None => None,
            }
        }
    });

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
        if image.is_some() || track.is_some() {
            div { style: "
                    display: flex;
                    flex-direction: column;
                    gap: 24px;
                    align-items: center;
                    justify-content: center;
                ",
                div { style: "
                        display: flex;
                        flex-direction: column;
                        gap: 16px;
                        align-items: center;
                        flex: 0 1 960px;
                        width: min(100%, 960px);
                    ",
                    if let Some(image) = image {
                        img {
                            src: "{image.url}",
                            width: image.width,
                            height: image.height,
                            style: "max-width: 100%; height: auto;",
                        }
                    }
                    div { style: "width: 100%;",
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
                                        canonical_history_chart.restart();
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
                div { style: "width: min(100%, 960px);",
                    match &*canonical_history_chart.read() {
                        Some(Some(Ok(chart))) => rsx! {
                            iframe {
                                title: "Canonical rating history",
                                srcdoc: "{chart}",
                                width: "100%",
                                height: "600",
                                scrolling: "no",
                                style: "border: 0; background: transparent; overflow: hidden;",
                            }
                        },
                        Some(Some(Err(error))) => rsx! {
                            p { "Failed to load canonical rating history: {error}" }
                        },
                        Some(None) => rsx! {
                            p { "No track playing." }
                        },
                        None => rsx! {
                            p { "Loading canonical rating history..." }
                        },
                    }
                }
            }
        }
    )
}

#[component]
fn HoverSlider(current_rating: Option<f32>, on_select: EventHandler<f64>) -> Element {
    let mut hovering = use_signal(|| false);
    let mut interacting = use_signal(|| false);
    let mut preview_rating = use_signal(|| None::<f64>);
    let mut width = use_signal(|| 1.0_f64); // avoid divide-by-zero
    let resting_progress = current_rating.unwrap_or(0.0).clamp(0.0, 5.0) as f64;
    let displayed_rating = preview_rating.read().unwrap_or(resting_progress);

    rsx! {
        div {
            style: "
                position: relative;
                width: min(400px, 100%);
                height: 48px;
                margin: 0 auto;
                background: #222;
                border-radius: 8px;
                overflow: hidden;
                cursor: pointer;
                user-select: none;
                touch-action: none;
            ",

            onmounted: move |evt| {
                spawn(async move {
                    if let Ok(rect) = evt.data().get_client_rect().await {
                        width.set(rect.size.width);
                    }
                });
            },

            onpointerenter: move |evt| {
                let width = *width.read();
                let x = evt.element_coordinates().x.clamp(0.0, width);
                let rating = ((x / width).clamp(0.0, 1.0)) * 5.0;
                hovering.set(true);
                preview_rating.set(Some(rating));
            },

            onpointerdown: move |evt| {
                let width = *width.read();
                let x = evt.element_coordinates().x.clamp(0.0, width);
                let rating = ((x / width).clamp(0.0, 1.0)) * 5.0;
                interacting.set(true);
                preview_rating.set(Some(rating));
            },

            onpointermove: move |evt| {
                if !*hovering.read() && !*interacting.read() {
                    return;
                }
                let width = *width.read();
                let x = evt.element_coordinates().x.clamp(0.0, width);
                let rating = ((x / width).clamp(0.0, 1.0)) * 5.0;
                preview_rating.set(Some(rating));
            },

            onpointerup: move |evt| {
                let width = *width.read();
                let x = evt.element_coordinates().x.clamp(0.0, width);
                let rating = ((x / width).clamp(0.0, 1.0)) * 5.0;
                interacting.set(false);
                preview_rating.set(if *hovering.read() { Some(rating) } else { None });
                on_select.call(rating);
            },

            onpointerleave: move |_| {
                hovering.set(false);
                if !*interacting.read() {
                    preview_rating.set(None);
                }
            },

            onpointercancel: move |_| {
                if let Some(rating) = *preview_rating.read() {
                    on_select.call(rating);
                }
                hovering.set(false);
                interacting.set(false);
                preview_rating.set(None);
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
                {format!("{displayed_rating:.2}")}
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
                    (displayed_rating / 5.0) * *width.read(),
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
        sanitize_chart_html(
            renderer
                .render(&chart)
                .expect("Rendering chart shouldn't fail"),
        )
    })
    .collect())
}

#[server]
async fn canonical_rating_history_chart(track_id: TrackId<'static>) -> Result<String> {
    use crate::spotify::{ratings_server, track_canonical_rating_history};
    use anyhow::anyhow;
    use charming::HtmlRenderer;

    let analyzation = ratings_server().await;
    let Some((track, analyzed)) = analyzation
        .tracks
        .iter()
        .find(|(track, _)| track.id.as_ref() == Some(&track_id))
    else {
        return Err(anyhow!("Track not found in ratings analyzation").into());
    };

    HtmlRenderer::new("Renderer", 960, 600)
        .render(&track_canonical_rating_history(track, analyzed))
        .map(sanitize_chart_html)
        .map_err(Into::into)
}

#[cfg(feature = "server")]
fn sanitize_chart_html(html: String) -> String {
    html.replace("<body>", "<body style=\"margin: 0; overflow: hidden;\">")
}
