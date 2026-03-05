use std::{
    collections::{BTreeMap, HashMap},
    fs::create_dir_all,
};

use crate::ratings::analyze::AnalyzedData;
use charming::{
    component::{Axis, Title},
    datatype::CompositeValue,
    element::{AreaStyle, AxisType, Color, ColorStop, LineStyle},
    series::Line,
    Chart, ImageRenderer,
};
use time::{Date, UtcDateTime};

pub fn visualize(data: AnalyzedData) {
    let mut vec = data
        .songs
        .iter()
        .map(|(song, analyzed)| (&song.name, analyzed.canonical_rating))
        .collect::<Vec<_>>();
    vec.sort_by(|(_, a), (_, b)| b.partial_cmp(a).unwrap());

    let n = 100000;
    println!("Top {n} Songs by Rating:");
    for (name, rating) in vec.iter().take(n) {
        println!("{name} - {rating}")
    }

    create_dir_all("charts").unwrap();

    canonical_rating_distribution(&data);
    average_rating_per_day(&data);
    // song_canonical_ratings(&data);
}

fn canonical_rating_distribution(data: &AnalyzedData) {
    let mut ratings = [0; 11];
    for rating in data
        .songs
        .iter()
        .flat_map(|(_, data)| data.rating_history.iter().map(|(rating, _)| *rating))
    {
        ratings[(rating * 2.) as usize] += 1;
    }

    // chart
    let chart = Chart::new()
        .title(Title::new().text("Canonical Rating Distribution"))
        .x_axis(Axis::new().type_(AxisType::Category).data(Vec::from_iter(
            (0..=10).map(|num| (num as f32 / 2.).to_string()),
        )))
        .y_axis(Axis::new().type_(AxisType::Value))
        .series(
            Line::new()
                .show_symbol(false)
                .line_style(LineStyle::new().width(0.0))
                .area_style(AreaStyle::new().color(linear_gradient()).opacity(0.8))
                .smooth(true)
                .data(ratings.to_vec()),
        );

    ImageRenderer::new(1920, 1080)
        .save(&chart, "charts/canonical_rating_distribution.svg")
        .unwrap();
}

fn average_rating_per_day(data: &AnalyzedData) {
    let ratings_per_day: BTreeMap<Date, Vec<f32>> = data
        .songs
        .iter()
        .flat_map(|(_, data)| data.rating_history.iter())
        .fold(BTreeMap::new(), |mut acc, (rating, date_time)| {
            let date = date_time.date();
            acc.entry(date).or_default().push(*rating);
            acc
        });

    let average_rating_per_day: Vec<Vec<CompositeValue>> = ratings_per_day
        .iter()
        .map(|(date, ratings)| {
            let average_rating = ratings.iter().map(f32::clone).sum::<f32>() / ratings.len() as f32;
            vec![date.to_string().into(), average_rating.into()]
        })
        .collect();

    let chart = Chart::new()
        .title(Title::new().text("Average Rating per Day"))
        .x_axis(Axis::new().type_(AxisType::Time))
        .y_axis(Axis::new().type_(AxisType::Value))
        .series(
            Line::new()
                // .show_symbol(false)
                .line_style(LineStyle::new().color(linear_gradient()))
                .smooth(true)
                .data(average_rating_per_day),
        );

    ImageRenderer::new(1920, 1080)
        .save(&chart, "charts/average_rating_per_day.svg")
        .unwrap();
}

fn linear_gradient() -> Color {
    Color::LinearGradient {
        x: 0.0,
        y: 0.0,
        x2: 0.0,
        y2: 1.0,
        color_stops: vec![
            ColorStop::new(0.0, "rgb(128, 255, 165)"),
            ColorStop::new(1.0, "rgb(1, 191, 236)"),
        ],
    }
}
