use std::{
    array,
    collections::{BTreeMap, HashMap},
    fs::create_dir_all,
    hash::{DefaultHasher, Hash, Hasher},
};

use crate::ratings::analyze::{canonical_rating, AnalyzedData};
#[cfg(feature = "server")]
use charming::{
    component::{Axis, Title},
    datatype::CompositeValue,
    element::{AreaStyle, AxisType, Color, ColorStop, LineStyle},
    series::Line,
    Chart, HtmlRenderer,
};
use time::{Date, UtcDateTime};

pub fn rating_per_song(data: AnalyzedData) {
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
}

#[cfg(feature = "server")]
pub fn canonical_rating_distribution(data: &AnalyzedData) -> Chart {
    let mut ratings = [0; 11];
    for rating in data
        .songs
        .iter()
        .flat_map(|(_, data)| data.rating_history.iter().map(|(rating, _)| *rating))
    {
        ratings[(rating * 2.) as usize] += 1;
    }

    // chart
    Chart::new()
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
        )
}

#[cfg(feature = "server")]
pub fn average_rating_per_day(data: &AnalyzedData) -> Chart {
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

    Chart::new()
        .title(Title::new().text("Average Rating per Day"))
        .x_axis(Axis::new().type_(AxisType::Time))
        .y_axis(Axis::new().type_(AxisType::Value))
        .series(
            Line::new()
                // .show_symbol(false)
                .line_style(LineStyle::new().color(linear_gradient()))
                .smooth(true)
                .data(average_rating_per_day),
        )
}

#[cfg(feature = "server")]
pub fn song_canonical_rating_histories(data: &AnalyzedData) -> Chart {
    use charming::element::{Formatter, JsFunction, Tooltip, Trigger};

    let now = UtcDateTime::now();
    let mut hasher = DefaultHasher::new();
    let chart = data
        .songs
        .iter()
        .map(|(song, analyzed)| {
            (
                song,
                (0..analyzed.rating_history.len())
                    .map(|i| {
                        (
                            analyzed.rating_history[i].1,
                            canonical_rating(&analyzed.rating_history[0..=i]),
                        )
                    })
                    .chain(std::iter::once((now, analyzed.canonical_rating))) // ensure all lines go to the end
                    .map(|(time, rating)| {
                        vec![
                            ((time.unix_timestamp_nanos() / 1_000_000) as i64).into(),
                            rating.into(),
                        ]
                    })
                    .collect::<Vec<Vec<CompositeValue>>>(),
            )
        })
        .fold(
            Chart::new()
                .title(Title::new().text("Canonical Rating Histories"))
                .x_axis(Axis::new().type_(AxisType::Time))
                .y_axis(Axis::new().type_(AxisType::Value))
                .tooltip(
                    Tooltip::new().trigger(Trigger::Axis), // .formatter(Formatter::Function(JsFunction::new_with_args(
                                                           //     "params",
                                                           //     "return params.map(p => p.seriesName).join('<br/>');",
                                                           // ))),
                ),
            |chart, data| {
                use charming::element::{Formatter, ItemStyle, Symbol, Tooltip, Trigger};

                data.0.name.hash(&mut hasher);
                let hash: u64 = hasher.finish();
                let [r, g, b] = array::from_fn(|i| ((hash >> i * 8) & 0xFF) as u8);
                let color = Color::Value(format!("rgb({r}, {g}, {b})"));

                chart.series(
                    Line::new()
                        .name(&data.0.name)
                        .tooltip(Tooltip::new().trigger(Trigger::Item).formatter(
                            Formatter::Function(JsFunction::new_with_args(
                                "params",
                                "return `${params.seriesName}: ${params.value[1].toFixed(2)}`",
                            )),
                        ))
                        // .show_symbol(false)
                        .item_style(ItemStyle::new().color(color.clone()))
                        .line_style(LineStyle::new().color(color))
                        .smooth(true)
                        .data(data.1),
                )
            },
        );

    chart
}

#[cfg(feature = "server")]
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
