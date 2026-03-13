use std::{
    array,
    hash::{DefaultHasher, Hash, Hasher},
};

use crate::ratings::Analyzation;
use charming::{
    Chart,
    component::{Axis, Legend, Title},
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
        .x_axis(Axis::new().type_(AxisType::Category).data(Vec::from_iter(
            (0..NUM_BINS).map(|num| format!("{:.2}-", num as f32 * BIN_SIZE,)),
        )))
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

pub fn song_canonical_rating_histories(data: &Analyzation) -> Chart {
    use charming::element::{Formatter, JsFunction, Tooltip, Trigger};

    let now = UtcDateTime::now();
    let mut hasher = DefaultHasher::new();
    data.tracks
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
        )
}

pub fn canonical_rating_correlations(data: &Analyzation) -> Chart {
    struct Point {
        x: f32,
        y: f32,
        name: String,
    }
    struct CorrelationSeries {
        name: &'static str,
        x_axis_name: &'static str,
        color: &'static str,
        points: Vec<Point>,
    }

    let duration = CorrelationSeries {
        name: "Duration (minutes)",
        x_axis_name: "Duration (minutes)",
        color: "rgb(59, 130, 246)",
        points: data
            .tracks
            .iter()
            .map(|(track, analyzed)| Point {
                x: (track.duration.num_milliseconds() as f32) / 60_000.0,
                y: analyzed.canonical_rating,
                name: track.name.clone(),
            })
            .collect(),
    };

    let popularity = CorrelationSeries {
        name: "Popularity",
        x_axis_name: "Popularity",
        color: "rgb(16, 185, 129)",
        points: data
            .tracks
            .iter()
            .map(|(track, analyzed)| Point {
                x: track.popularity as f32,
                y: analyzed.canonical_rating,
                name: track.name.clone(),
            })
            .collect(),
    };

    #[derive(Clone, Copy)]
    struct XY {
        x: f32,
        y: f32,
    }
    #[derive(Clone, Copy)]
    struct RegressionLineWithCorrelation {
        min: XY,
        max: XY,
        correlation: f32,
    }

    fn regression_line_with_correlation(points: &[Point]) -> Option<RegressionLineWithCorrelation> {
        let n = points.len() as f64;
        if n < 2.0 {
            return None;
        }

        let (sum_x, sum_y) = points
            .iter()
            .fold((0.0_f64, 0.0_f64), |(sx, sy), Point { x, y, .. }| {
                (sx + *x as f64, sy + *y as f64)
            });
        let mean_x = sum_x / n;
        let mean_y = sum_y / n;

        let (covariance, variance_x, variance_y) = points.iter().fold(
            (0.0_f64, 0.0_f64, 0.0_f64),
            |(cov, var_x, var_y), Point { x, y, .. }| {
                let dx = *x as f64 - mean_x;
                let dy = *y as f64 - mean_y;
                (cov + dx * dy, var_x + dx * dx, var_y + dy * dy)
            },
        );

        if variance_x <= f64::EPSILON || variance_y <= f64::EPSILON {
            return None;
        }

        let slope = covariance / variance_x;
        let intercept = mean_y - slope * mean_x;

        let (min_x, max_x) = points
            .iter()
            .map(|point| point.x)
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(min, max), x| {
                (min.min(x), max.max(x))
            });

        if !min_x.is_finite() || !max_x.is_finite() {
            return None;
        }

        let start_y = (slope * min_x as f64 + intercept) as f32;
        let end_y = (slope * max_x as f64 + intercept) as f32;
        let correlation = (covariance / (variance_x.sqrt() * variance_y.sqrt())) as f32;

        Some(RegressionLineWithCorrelation {
            min: XY {
                x: min_x,
                y: start_y,
            },
            max: XY { x: max_x, y: end_y },
            correlation,
        })
    }

    let series = [duration, popularity]
        .into_iter()
        .map(|series| {
            let regression = regression_line_with_correlation(&series.points);
            let correlation = regression.map(|val| val.correlation);
            let legend_label = match correlation {
                Some(correlation) => format!("{} (r={correlation:.4})", series.name),
                None => series.name.to_string(),
            };
            (series, regression, legend_label)
        })
        .collect::<Vec<_>>();

    let mut chart = base_chart()
        .title(Title::new().text("Canonical Rating Correlations"))
        .tooltip(Tooltip::new().trigger(Trigger::Item))
        .x_axis(
            Axis::new()
                .type_(AxisType::Value)
                .name(series[0].0.x_axis_name),
        )
        .x_axis(
            Axis::new()
                .type_(AxisType::Value)
                .name(series[1].0.x_axis_name)
                .position("top"),
        )
        .y_axis(
            Axis::new()
                .type_(AxisType::Value)
                .name("Canonical Rating")
                .min(0.0)
                .max(5.0),
        )
        .legend(
            Legend::new().data(
                series
                    .iter()
                    .map(|(_, _, label)| label.clone())
                    .collect::<Vec<_>>(),
            ),
        );

    for (index, (series, regression, legend_label)) in series.into_iter().enumerate() {
        let scatter_data = series
            .points
            .iter()
            .map(|&Point { x, y, ref name }| vec![x.into(), y.into(), name.clone().into()])
            .collect::<Vec<Vec<CompositeValue>>>();

        chart = chart.series(
            Line::new()
                .name(&legend_label)
                .show_symbol(true)
                .line_style(LineStyle::new().width(0.0))
                .item_style(ItemStyle::new().color(Color::Value(series.color.into())))
                .tooltip(Tooltip::new().trigger(Trigger::Item).formatter(
                    Formatter::Function(JsFunction::new_with_args(
                        "params",
                        &format!(
                            "const [x, y, name] = params.data; return `${{name}}<br/>{}: ${{Number(x).toFixed(2)}}<br/>Canonical Rating: ${{Number(y).toFixed(2)}}`;",
                            series.x_axis_name
                        ),
                    )),
                ))
                .x_axis_index(index as f64)
                .data(scatter_data),
        );

        if let Some(RegressionLineWithCorrelation {
            min: XY {
                x: start_x,
                y: start_y,
            },
            max: XY { x: end_x, y: end_y },
            ..
        }) = regression
        {
            chart = chart.series(
                Line::new()
                    .name(&legend_label)
                    .show_symbol(false)
                    .line_style(
                        LineStyle::new()
                            .color(Color::Value(series.color.into()))
                            .width(2.0),
                    )
                    .x_axis_index(index as f64)
                    .data(vec![
                        vec![CompositeValue::from(start_x), CompositeValue::from(start_y)],
                        vec![CompositeValue::from(end_x), CompositeValue::from(end_y)],
                    ]),
            );
        }
    }

    chart
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

fn to_composite_values(
    (time, value): (UtcDateTime, impl Into<CompositeValue>),
) -> Vec<CompositeValue> {
    vec![
        ((time.unix_timestamp_nanos() / 1_000_000) as i64).into(),
        value.into(),
    ]
}

pub fn base_chart() -> Chart {
    use charming::component::{Feature, Toolbox, ToolboxDataZoom};

    // TODO: see if I can make this preserve lines between off-screen and on-screen datapoints when zooming
    Chart::new().toolbox(Toolbox::new().feature(Feature::new().data_zoom(ToolboxDataZoom::new())))
}
