use crate::{config::Config, ratings::Data};
use dioxus::prelude::*;

#[component]
pub fn Spotify() -> Element {
    let charts = use_server_future(move || async move { charts().await })?;
    rsx!(match &*charts.read_unchecked() {
        Some(Ok(charts)) => {
            rsx!(for (i, chart) in charts.iter().enumerate() {
                iframe {
                    width: 1920,
                    height: 1080,
                    srcdoc: "{chart}",
                }
            })
        }
        Some(Err(e)) => rsx! { "Error loading charts: {e}" },
        None => rsx! {"Loading charts..."},
    })
}

#[server]
async fn charts() -> Result<[String; 3]> {
    use crate::{
        config::config,
        ratings::{
            average_rating_per_day, canonical_rating_distribution, song_canonical_rating_histories,
        },
    };
    use charming::HtmlRenderer;

    let config = config().await;

    let json = tokio::fs::read_to_string(config.spotify.export_json_path).await?;
    let raw_data: Data = serde_json::from_str(&json)?;
    let analyzed_data = raw_data.analyze();

    let renderer = HtmlRenderer::new("Renderer", 1920, 1080);

    Ok([
        canonical_rating_distribution,
        average_rating_per_day,
        song_canonical_rating_histories,
    ]
    .map(|f| {
        let chart = f(&analyzed_data);
        renderer
            .render(&chart)
            .expect("Rendering chart shouldn't fail")
    }))
}
