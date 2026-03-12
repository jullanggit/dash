use std::{
    array,
    hash::{DefaultHasher, Hash, Hasher},
};

#[cfg(feature = "server")]
use crate::ratings::Analyzation;
#[cfg(feature = "server")]
use charming::{
    Chart,
    component::{Axis, Title},
    datatype::CompositeValue,
    element::{
        AreaStyle, AxisType, Color, ColorStop, Formatter, ItemStyle, JsFunction, LineStyle,
        Tooltip, Trigger,
    },
    series::Line,
};

use time::UtcDateTime;

pub fn rating_per_song(data: Analyzation) {
    let mut vec = data
        .tracks
        .iter()
        .map(|(song, analyzed)| (&song.name, analyzed.canonical_rating))
        .collect::<Vec<_>>();
    vec.sort_unstable_by(|(_, a), (_, b)| b.partial_cmp(a).unwrap());

    let n = 100000;
    println!("Top {n} Songs by Rating:");
    for (name, rating) in vec.iter().take(n) {
        println!("{name} - {rating}")
    }
}

#[cfg(feature = "server")]
pub fn canonical_rating_distribution(data: &Analyzation) -> Chart {
    use crate::ratings::TrackAnalyzation;

    const BIN_SIZE: f32 = 0.25;
    const NUM_BINS: usize = (5. / BIN_SIZE) as usize;
    let mut bins = [0_i64; NUM_BINS];

    for (
        _,
        TrackAnalyzation {
            canonical_rating, ..
        },
    ) in &data.tracks
    {
        let bin_index = ((*canonical_rating / BIN_SIZE) as usize).min(NUM_BINS - 1);
        bins[bin_index] += 1;
    }

    // chart
    base_chart()
        .title(Title::new().text("Canonical Rating Distribution"))
        .x_axis(
            Axis::new()
                .type_(AxisType::Category)
                .data(Vec::from_iter((0..NUM_BINS).map(|num| {
                    format!(
                        "{:.2}-{:.2}",
                        num as f32 * BIN_SIZE,
                        (num + 1) as f32 * BIN_SIZE
                    )
                }))),
        )
        .y_axis(Axis::new().type_(AxisType::Value))
        .series(
            Line::new()
                .show_symbol(false)
                .line_style(LineStyle::new().width(0.0))
                .area_style(AreaStyle::new().color(linear_gradient()).opacity(0.8))
                .smooth(true)
                .data(bins.to_vec()),
        )
}

#[cfg(feature = "server")]
pub fn average_rating_per_day(data: &Analyzation) -> Chart {
    base_chart()
        .title(Title::new().text("Average Rating per Day"))
        .x_axis(Axis::new().type_(AxisType::Time))
        .y_axis(Axis::new().type_(AxisType::Value))
        .series(
            Line::new()
                .show_symbol(false)
                .line_style(LineStyle::new().color(linear_gradient()))
                .smooth(true)
                .data(
                    data.average_rating_per_day
                        .iter()
                        .map(|&(date, value)| {
                            vec![CompositeValue::from(date.to_string()), value.into()]
                        })
                        .collect(),
                ),
        )
}

// TODO: make sure lines go to the end with new analyzations
#[cfg(feature = "server")]
pub fn num_ratings_history(data: &Analyzation) -> Chart {
    let convert = |&(date_time, count)| (date_time, count as i64);
    base_chart()
        .title(Title::new().text("Num Ratings"))
        .x_axis(Axis::new().type_(AxisType::Time))
        .y_axis(Axis::new().type_(AxisType::Value))
        // TODO: maybe ensure that all points have a value from both series
        .tooltip(
            Tooltip::new()
                .trigger(Trigger::Axis)
                .formatter(Formatter::Function(JsFunction::new_with_args(
                    "params",
                    "console.log(params); return params.map(p => `${p.seriesName}: ${p.dataIndex}`).join('<br/>');",
                ))),
        )
        .series(
            Line::new()
                .name("Total number of Ratings")
                .show_symbol(false)
                .line_style(LineStyle::new().color(linear_gradient()))
                .smooth(true)
                .data(data.num_ratings_history.iter().map(convert).map(to_composite_values).collect()),
        )
        .series(
            Line::new()
                .name("Number of rated Songs")
                .show_symbol(false)
                .line_style(LineStyle::new().color(linear_gradient2()))
                .smooth(true)
                .data(data.num_rated_tracks_history.iter().map(convert).map(to_composite_values).collect()),
        )
}

#[cfg(feature = "server")]
pub fn song_canonical_rating_histories(data: &Analyzation) -> Chart {
    use charming::element::{Formatter, JsFunction, Tooltip, Trigger};

    let now = UtcDateTime::now();
    let mut hasher = DefaultHasher::new();
    let chart = data
        .tracks
        .iter()
        .map(|(song, analyzed)| {
            (
                song,
                analyzed
                    .canonical_rating_history
                    .iter()
                    .cloned()
                    .chain(std::iter::once((now, analyzed.canonical_rating))) // ensure all lines go to the end
                    .map(to_composite_values)
                    .collect::<Vec<Vec<CompositeValue>>>(),
            )
        })
        .fold(
            base_chart()
                .title(Title::new().text("Canonical Rating Histories"))
                .x_axis(Axis::new().type_(AxisType::Time))
                .y_axis(Axis::new().type_(AxisType::Value).min(0.0).max(5.0))
                .tooltip(
                    Tooltip::new().trigger(Trigger::Axis), // .formatter(Formatter::Function(JsFunction::new_with_args(
                                                           //     "params",
                                                           //     "return params.map(p => p.seriesName).join('<br/>');",
                                                           // ))),
                ),
            |chart, (track, history)| {
                track.name.hash(&mut hasher);
                let hash: u64 = hasher.finish();
                let [r, g, b] = array::from_fn(|i| ((hash >> (i * 8)) & 0xFF) as u8);
                let color = Color::Value(format!("rgb({r}, {g}, {b})"));

                chart.series(
                    Line::new()
                        .name(&track.name)
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
                        .data(history),
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

#[cfg(feature = "server")]
fn linear_gradient2() -> Color {
    Color::LinearGradient {
        x: 0.0,
        y: 0.0,
        x2: 0.0,
        y2: 1.0,
        color_stops: vec![
            ColorStop::new(0.0, "rgb(59, 130, 246)"),
            ColorStop::new(1.0, "rgb(147, 51, 234)"),
        ],
    }
}

#[cfg(feature = "server")]
fn to_composite_values(
    (time, value): (UtcDateTime, impl Into<CompositeValue>),
) -> Vec<CompositeValue> {
    vec![
        ((time.unix_timestamp_nanos() / 1_000_000) as i64).into(),
        value.into(),
    ]
}

#[cfg(feature = "server")]
pub fn base_chart() -> Chart {
    use charming::component::{Feature, Toolbox, ToolboxDataZoom};

    // TODO: see if I can make this preserve lines between off-screen and on-screen datapoints when zooming
    Chart::new().toolbox(Toolbox::new().feature(Feature::new().data_zoom(ToolboxDataZoom::new())))
}
