use crate::{
    assert_authenticated,
    spotify::{
        add_rating, caching::use_server_fn, genres, playback_options,
        rating_if_recently_rated as fetch_rating, use_playback_state, weighted_playback,
    },
};
use dioxus::prelude::*;
use rspotify_model::{
    Context, CurrentPlaybackContext, FullTrack, PlayableItem, PlaylistId, TrackId, Type,
};
use time::Duration;

const MOBILE_BREAKPOINT_PX: u16 = 768;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SubmitStatus {
    Success,
    Error,
}

#[component]
pub fn Spotify() -> Element {
    let playback_state = use_playback_state();
    let is_mobile = use_is_mobile();
    let mobile_mode = is_mobile() != Some(false);
    rsx!(
        Player { playback_state, mobile_mode }
        if !mobile_mode {
            DesktopCharts {}
        }
    )
}

#[component]
fn DesktopCharts() -> Element {
    let charts = use_server_fn(charts, Duration::MINUTE);

    rsx! {
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
    }
}

#[component]
fn Player(
    playback_state: Signal<Option<Option<CurrentPlaybackContext>>>,
    mobile_mode: bool,
) -> Element {
    let playback_options = use_resource(|| async move { playback_options().await });
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
    let mut submit_status = use_signal(|| None::<(TrackId<'static>, SubmitStatus)>);
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
    let current_playlist_id = state.and_then(|state| match &state.context {
        Some(Context {
            _type: Type::Playlist,
            uri,
            ..
        }) => PlaylistId::from_id(uri).ok().map(PlaylistId::into_static),
        _ => None,
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
        Some(Some(Ok(rating))) => *rating,
        _ => None,
    };
    let current_rating = pending_rating_value.or(fetched_rating);
    let current_submit_status =
        submit_status
            .read()
            .as_ref()
            .and_then(|(submitted_track_id, status)| {
                (Some(submitted_track_id) == current_track_id.as_ref()).then_some(*status)
            });
    let slider_key = current_track_id
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_else(|| "no-track".to_string());
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
                        flex: 0 1 640px;
                        width: min(100%, 640px);
                    ",
                    if mobile_mode {
                        if let Some(image) = image {
                            img {
                                src: "{image.url}",
                                width: image.width,
                                height: image.height,
                                style: "max-width: 100%; height: auto;",
                            }
                        }
                    } else if let Some(image) = image {
                        div { style: "
                                display: grid;
                                grid-template-columns: minmax(0, 1fr) auto minmax(0, 1fr);
                                align-items: center;
                                width: min(100vw, 1200px);
                                gap: 24px;
                            ",
                            div {}
                            img {
                                src: "{image.url}",
                                width: image.width,
                                height: image.height,
                                style: "max-width: min(100%, 640px); height: auto; justify-self: center;",
                            }
                            PlaybackOptionsPanel { current_playlist_id, playback_options }
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
                            key: "{slider_key}",
                            current_rating,
                            mobile_mode,
                            submit_status: current_submit_status,
                            on_select: move |selected_rating| async move {
                                let Some(track_id) = track_id.read().clone() else {
                                    return;
                                };
                                let track_id = track_id.into_static();
                                match add_rating(track_id.clone(), selected_rating as f32).await {
                                    Ok(canonical_rating) => {
                                        pending_rating.set(Some((track_id.clone(), canonical_rating)));
                                        submit_status.set(Some((track_id, SubmitStatus::Success)));
                                        rating.restart();
                                        canonical_history_chart.restart();
                                    }
                                    Err(error) => {
                                        pending_rating.set(None);
                                        submit_status.set(Some((track_id, SubmitStatus::Error)));
                                        error!("Failed to submit rating: {error}");
                                    }
                                }
                            },
                        }
                    }
                }
                div { style: "width: min(100%, 960px);",
                    match &*canonical_history_chart.read() {
                        Some(Some(Ok(Some(chart)))) => rsx! {
                            iframe {
                                title: "Canonical rating history",
                                srcdoc: "{chart}",
                                width: "100%",
                                height: "600",
                                scrolling: "no",
                                style: "border: 0; background: transparent; overflow: hidden;",
                            }
                        },
                        Some(Some(Ok(None))) => rsx! {},
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
fn HoverSlider(
    current_rating: Option<f32>,
    mobile_mode: bool,
    submit_status: Option<SubmitStatus>,
    on_select: EventHandler<f64>,
) -> Element {
    let mut hovering = use_signal(|| false);
    let mut interacting = use_signal(|| false);
    let mut preview_rating = use_signal(|| None::<f64>);
    let mut width = use_signal(|| 1.0_f64); // avoid divide-by-zero
    let resting_progress = current_rating.map(|rating| rating.clamp(0.0, 5.0) as f64);
    let displayed_rating = preview_rating.read().or(resting_progress);
    let submit_label = match submit_status {
        Some(SubmitStatus::Success) => "Success",
        Some(SubmitStatus::Error) => "Error",
        None => "Submit",
    };
    let submit_background = match submit_status {
        Some(SubmitStatus::Success) => "#1db954",
        Some(SubmitStatus::Error) => "#d64545",
        None => "#f59e0b",
    };

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
                if mobile_mode {
                    preview_rating.set(Some(rating));
                } else {
                    preview_rating.set(if *hovering.read() { Some(rating) } else { None });
                    on_select.call(rating);
                }
            },

            onpointerleave: move |_| {
                hovering.set(false);
                if !mobile_mode && !*interacting.read() {
                    preview_rating.set(None);
                }
            },

            onpointercancel: move |_| {
                if mobile_mode {
                    interacting.set(false);
                } else {
                    if let Some(rating) = *preview_rating.read() {
                        on_select.call(rating);
                    }
                    hovering.set(false);
                    interacting.set(false);
                    preview_rating.set(None);
                }
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
                {displayed_rating.map(|rating| format!("{rating:.2}"))}
            }

            if let Some(displayed_rating) = displayed_rating {
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
        if mobile_mode {
            button {
                style: "
                    width: min(400px, 100%);
                    margin-top: 12px;
                    padding: 12px 16px;
                    border-radius: 8px;
                    background: {submit_background};
                    color: #111;
                    font-weight: 700;
                ",
                onclick: move |_| {
                    let rating = preview_rating.read().or(resting_progress).unwrap_or(0.0);
                    on_select.call(rating);
                },
                "{submit_label}"
            }
        }
    }
}

#[component]
fn PlaybackOptionsPanel(
    current_playlist_id: Option<PlaylistId<'static>>,
    playback_options: Resource<Result<crate::spotify::playback::PlaybackOptions>>,
) -> Element {
    let is_enabled = match &*playback_options.read() {
        Some(Ok(options)) => current_playlist_id
            .as_ref()
            .is_some_and(|playlist_id| options.weighted_playback_enabled(playlist_id)),
        _ => false,
    };
    let is_pending = playback_options.pending();
    let helper_text = match current_playlist_id {
        Some(_) => {
            if is_pending {
                "Saving playback options..."
            } else {
                "Bias queueing toward higher-rated tracks in this playlist."
            }
        }
        None => "Weighted playback is only available while listening to a playlist.",
    };

    rsx! {
        div { style: "
                justify-self: start;
                width: min(100%, 320px);
                padding: 20px;
                border: 1px solid #2f2f2f;
                border-radius: 16px;
                background: rgba(18, 18, 18, 0.92);
            ",
            h3 { style: "margin-bottom: 16px;", "Playback Options" }
            div { style: "
                    display: flex;
                    align-items: center;
                    justify-content: space-between;
                    gap: 16px;
                ",
                div {
                    p { style: "font-weight: 600; margin: 0;", "Weighted Playback" }
                    p { style: "font-size: 0.9rem; color: #b3b3b3; margin: 6px 0 0;",
                        "{helper_text}"
                    }
                }
                button {
                    role: "switch",
                    "aria-checked": "{is_enabled}",
                    disabled: current_playlist_id.is_none() || is_pending,
                    style: format!(
                        "
                                                                            position: relative;
                                                                            width: 52px;
                                                                            height: 30px;
                                                                            border-radius: 999px;
                                                                            border: 0;
                                                                            padding: 0;
                                                                            background: {};
                                                                            opacity: {};
                                                                            cursor: {};
                                                                        ",
                        if is_enabled { "#1db954" } else { "#4b5563" },
                        if current_playlist_id.is_some() && !is_pending { "1" } else { "0.55" },
                        if current_playlist_id.is_some() && !is_pending {
                            "pointer"
                        } else {
                            "not-allowed"
                        },
                    ),
                    onclick: move |_| {
                        let playlist_id = current_playlist_id.clone();
                        async move {
                            let Some(playlist_id) = playlist_id else {
                                return;
                            };
                            if let Err(error) = weighted_playback(playlist_id, !is_enabled).await {
                                error!("Failed to update playback options: {error}");
                            }
                            playback_options.restart();
                        }
                    },
                    span {
                        style: format!(
                            "
                                                                                            position: absolute;
                                                                                            top: 3px;
                                                                                            left: 3px;
                                                                                            width: 24px;
                                                                                            height: 24px;
                                                                                            border-radius: 50%;
                                                                                            background: white;
                                                                                            transform: translateX({});
                                                                                            transition: transform 120ms ease;
                                                                                        ",
                            if is_enabled { "22px" } else { "0" },
                        ),
                    }
                }
            }
        }
    }
}

fn use_is_mobile() -> Signal<Option<bool>> {
    let mut is_mobile = use_signal(|| None);

    use_effect(move || {
        spawn(async move {
            let eval = document::eval(&format!(
                "return window.matchMedia('(max-width: {}px)').matches;",
                MOBILE_BREAKPOINT_PX
            ));
            is_mobile.set(eval.join::<bool>().await.ok());
        });
    });

    is_mobile
}

#[server]
async fn charts() -> Result<Vec<String>> {
    use crate::spotify::{
        artist_proportions, average_rating_per_day, canonical_rating_correlations,
        canonical_rating_distribution, genre_proportions, num_ratings_history, ratings_server,
        song_canonical_rating_histories,
    };
    use charming::HtmlRenderer;

    assert_authenticated!();

    let renderer = HtmlRenderer::new("Renderer", 1920, 1080);

    let analyzation = ratings_server().await;

    Ok([
        genre_proportions,
        artist_proportions,
        canonical_rating_distribution,
        num_ratings_history,
        song_canonical_rating_histories,
        canonical_rating_correlations,
        average_rating_per_day,
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
async fn canonical_rating_history_chart(track_id: TrackId<'static>) -> Result<Option<String>> {
    use crate::spotify::{ratings_server, track_canonical_rating_history};
    use charming::HtmlRenderer;

    assert_authenticated!();

    let analyzation = ratings_server().await;
    let Some((track, analyzed)) = analyzation
        .tracks
        .iter()
        .find(|(track, _)| track.id.as_ref() == Some(&track_id))
    else {
        return Ok(None);
    };

    if analyzed.rating_history.is_empty() {
        return Ok(None);
    }

    HtmlRenderer::new("Renderer", 960, 600)
        .render(&track_canonical_rating_history(track, analyzed))
        .map(sanitize_chart_html)
        .map(Some)
        .map_err(Into::into)
}

#[cfg(feature = "server")]
fn sanitize_chart_html(html: String) -> String {
    html.replace("<body>", "<body style=\"margin: 0; overflow: hidden;\">")
}
