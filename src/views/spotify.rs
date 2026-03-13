use dioxus::prelude::*;

#[component]
pub fn Spotify() -> Element {
    let charts = use_server_future(move || async move { charts().await })?;
    rsx!(
        h2 { "Control" }
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

#[server]
async fn charts() -> Result<[String; 5]> {
    use crate::ratings::{
        average_rating_per_day, canonical_rating_correlations, canonical_rating_distribution,
        num_ratings_history, ratings, song_canonical_rating_histories,
    };

    use charming::HtmlRenderer;

    let renderer = HtmlRenderer::new("Renderer", 1920, 1080);

    let analyzation = ratings().await;

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
