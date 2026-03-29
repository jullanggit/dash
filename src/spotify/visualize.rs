use crate::spotify::{
    analyze::{Analyzation, TrackAnalyzation},
    playback::weight,
};
use charming::{
    Chart,
    component::{Axis, Legend, Title},
    datatype::CompositeValue,
    element::{
        AreaStyle, AxisType, Color, ColorStop, Formatter, ItemStyle, JsFunction, LineStyle,
        SplitLine, Tooltip, Trigger,
    },
    series::{Line, Pie, PieRoseType},
};
use rspotify_model::FullTrack;
use std::{
    array,
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
};
use time::{Date, Month, UtcDateTime};

const TOP_PROPORTIONS: usize = 100;
const MAX_CANONICAL_RATING: f32 = 5.0;

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
    const SAMPLE_STEP: f32 = 0.01;
    const BANDWIDTH: f32 = 0.10;

    fn distribution_density(ratings: impl IntoIterator<Item = f32>) -> Vec<f32> {
        let normalization = 1.0 / (BANDWIDTH * (2.0 * std::f32::consts::PI).sqrt());
        let ratings = ratings.into_iter().collect::<Vec<_>>();
        let samples = (MAX_CANONICAL_RATING / SAMPLE_STEP).round() as usize;

        if ratings.is_empty() {
            return vec![0.0; samples + 1];
        }

        (0..=samples)
            .map(|index| {
                let center = index as f32 * SAMPLE_STEP;
                ratings
                    .iter()
                    .map(|rating| {
                        let standardized = (center - *rating) / BANDWIDTH;
                        (-0.5 * standardized.powi(2)).exp() * normalization
                    })
                    .sum::<f32>()
                    / ratings.len() as f32
            })
            .collect()
    }

    let distribution: Vec<Vec<CompositeValue>> = distribution_density(data.tracks.iter().map(
        |(
            _,
            TrackAnalyzation {
                canonical_rating, ..
            },
        )| *canonical_rating,
    ))
    .into_iter()
    .enumerate()
    .map(|(index, density)| {
        let center = index as f32 * SAMPLE_STEP;
        vec![CompositeValue::from(center), CompositeValue::from(density)]
    })
    .collect::<Vec<_>>();

    // chart
    base_chart()
        .title(Title::new().text("Canonical Rating Distribution"))
        .x_axis(
            Axis::new()
                .type_(AxisType::Value)
                .min(0.0)
                .max(5.0)
                .interval(0.5),
        )
        .y_axis(
            Axis::new()
                .type_(AxisType::Value)
                .axis_label(charming::element::AxisLabel::new().show(false))
                .split_line(SplitLine::new().show(false)),
        )
        .series(
            Line::new()
                .show_symbol(false)
                .line_style(LineStyle::new().width(0.0))
                .area_style(AreaStyle::new().color(linear_gradient()).opacity(0.8))
                .data(distribution),
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

fn canonical_rating_history_chart<'a>(
    tracks: impl IntoIterator<Item = (&'a FullTrack, &'a TrackAnalyzation)>,
    title: &'static str,
) -> Chart {
    use charming::element::{Formatter, JsFunction, Tooltip, Trigger};

    let now = UtcDateTime::now();
    let mut hasher = DefaultHasher::new();
    tracks
        .into_iter()
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
                .title(Title::new().text(title))
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

pub fn song_canonical_rating_histories(data: &Analyzation) -> Chart {
    canonical_rating_history_chart(
        data.tracks
            .iter()
            .map(|(track, analyzed)| (track, analyzed)),
        "Canonical Rating Histories",
    )
}

pub fn track_canonical_rating_history(track: &FullTrack, analyzed: &TrackAnalyzation) -> Chart {
    canonical_rating_history_chart([(track, analyzed)], "Canonical Rating History")
}

pub fn canonical_rating_correlations(data: &Analyzation) -> Chart {
    #[derive(Debug)]
    struct Point {
        x: f32,
        y: f32,
        name: String,
        release_date: String,
    }
    #[derive(Debug)]
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
                release_date: track
                    .album
                    .release_date
                    .clone()
                    .unwrap_or_else(|| "Unknown".to_string()),
            })
            .collect(),
    };

    // Apparently spotify removed this field, so this might stop working at some point
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
                release_date: track
                    .album
                    .release_date
                    .clone()
                    .unwrap_or_else(|| "Unknown".to_string()),
            })
            .collect(),
    };

    let release_date = CorrelationSeries {
        name: "Release Date",
        x_axis_name: "Release Date",
        color: "rgb(245, 158, 11)",
        points: data
            .tracks
            .iter()
            .filter_map(|(track, analyzed)| {
                let release_date = track.album.release_date.clone()?;
                let timestamp = release_date_to_timestamp_millis(
                    &release_date,
                    track.album.release_date_precision.as_deref(),
                )?;

                Some(Point {
                    x: timestamp as f32,
                    y: analyzed.canonical_rating,
                    name: track.name.clone(),
                    release_date,
                })
            })
            .collect(),
    };

    #[derive(Clone, Copy, Debug)]
    struct XY {
        x: f32,
        y: f32,
    }
    #[derive(Clone, Copy, Debug)]
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

    let series = [duration, popularity, release_date]
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
                .split_line(SplitLine::new().show(false))
                .name(series[0].0.x_axis_name),
        )
        .x_axis(
            Axis::new()
                .type_(AxisType::Value)
                .name(series[1].0.x_axis_name)
                .split_line(SplitLine::new().show(false))
                .position("top"),
        )
        .x_axis(
            Axis::new()
                .type_(AxisType::Time)
                .name(series[2].0.x_axis_name)
                .position("bottom")
                .split_line(SplitLine::new().show(false))
                .split_number(5)
                .offset(30.0),
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
            .map(
                |Point {
                     x,
                     y,
                     name,
                     release_date,
                 }| {
                    vec![
                        (*x).into(),
                        (*y).into(),
                        name.clone().into(),
                        release_date.clone().into(),
                    ]
                },
            )
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
                            "const [x, y, name, releaseDate] = params.data; const xValue = {} === 'Release Date' ? releaseDate : Number(x).toFixed(2); return `${{name}}<br/>{}: ${{xValue}}<br/>Canonical Rating: ${{Number(y).toFixed(2)}}`;",
                            serde_json::to_string(series.x_axis_name)
                                .expect("series axis names should serialize"),
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

fn sort_and_limit(mut values: Vec<(f32, String)>) -> Vec<(f32, String)> {
    values.sort_unstable_by(|(a, _), (b, _)| b.total_cmp(a));
    values.truncate(TOP_PROPORTIONS);
    values
}

fn proportion_pie(center: &str, data: Vec<(f32, String)>, name: &str) -> Pie {
    Pie::new()
        .rose_type(PieRoseType::Radius)
        .radius(vec!["40", "150"])
        .center(vec![center, "50%"])
        .item_style(ItemStyle::new().border_radius(8))
        .data(data)
        .name(name)
}

pub fn genre_proportions(data: &Analyzation) -> Chart {
    let mut genre_counts: HashMap<String, (f32, u32)> = HashMap::new();

    for (_, analyzation) in &data.tracks {
        for genre in &analyzation.genres {
            let (acc, num) = genre_counts.entry(genre.clone()).or_insert((0.0, 0));
            *acc += weight(analyzation.canonical_rating);
            *num += 1;
        }
    }

    let cumulative_genre_counts = genre_counts
        .clone()
        .into_iter()
        .map(|(genre, (acc, _))| (acc, genre))
        .collect::<Vec<_>>();

    let average_genre_counts = genre_counts
        .into_iter()
        .map(|(genre, (acc, num))| (acc / num as f32, genre))
        .collect::<Vec<_>>();

    let max = |collection: &[(f32, String)]| -> f32 {
        collection
            .iter()
            .map(|(num, _)| *num)
            .max_by(|a, b| a.total_cmp(b))
            .unwrap_or(1.0)
    };

    let max_cumulative = max(&cumulative_genre_counts);
    let max_average = max(&average_genre_counts);
    let genre_scores = cumulative_genre_counts
        .iter()
        .zip(&average_genre_counts)
        .map(|((cumulative, genre), (average, genre2))| {
            assert_eq!(genre, genre2);

            (
                *cumulative / max_cumulative + *average / max_average,
                genre.clone(),
            )
        })
        .collect::<Vec<_>>();

    let cumulative_genre_counts = sort_and_limit(cumulative_genre_counts);
    let average_genre_counts = sort_and_limit(average_genre_counts);
    let genre_scores = sort_and_limit(genre_scores);

    base_chart()
        .title(Title::new().text("Genres"))
        .tooltip(Tooltip::new().trigger(Trigger::Item))
        .series(proportion_pie("18%", cumulative_genre_counts, "Cumulative"))
        .series(proportion_pie("50%", genre_scores, "Score"))
        .series(proportion_pie("82%", average_genre_counts, "Average"))
}

pub fn artist_proportions(data: &Analyzation) -> Chart {
    let mut artist_counts: HashMap<String, f32> = HashMap::new();
    let mut song_ratings: HashMap<_, f32> = HashMap::new();

    for (track, analyzation) in &data.tracks {
        let track_weight = weight(analyzation.canonical_rating);
        song_ratings.insert(
            (track.name.clone(), track.id.clone()),
            weight(analyzation.canonical_rating),
        );

        for artist in &track.artists {
            *artist_counts.entry(artist.name.clone()).or_insert(0.0) += track_weight;
        }
    }

    let cumulative_artist_counts = sort_and_limit(
        artist_counts
            .into_iter()
            .map(|(artist, acc)| (acc, artist))
            .collect(),
    );
    let song_ratings = sort_and_limit(
        song_ratings
            .into_iter()
            .map(|((name, id), rating)| (rating, name))
            .collect(),
    );

    base_chart()
        .title(Title::new().text("Song Rating and Artist Cumulative Rating"))
        .tooltip(Tooltip::new().trigger(Trigger::Item))
        .series(proportion_pie("25%", song_ratings, "Songs"))
        .series(proportion_pie("75%", cumulative_artist_counts, "Artists"))
}

fn release_date_to_timestamp_millis(
    release_date: &str,
    release_date_precision: Option<&str>,
) -> Option<i64> {
    let inferred_precision = match release_date.len() {
        4 => "year",
        7 => "month",
        10 => "day",
        _ => return None,
    };
    let precision = release_date_precision.unwrap_or(inferred_precision);

    let mut parts = release_date.split('-');
    let year = parts.next()?.parse().ok()?;
    let month = match precision {
        "year" => Month::January,
        "month" | "day" => Month::try_from(parts.next()?.parse::<u8>().ok()?).ok()?,
        _ => return None,
    };
    let day = match precision {
        "day" => parts.next()?.parse().ok()?,
        "year" | "month" => 1,
        _ => return None,
    };

    let date = Date::from_calendar_date(year, month, day).ok()?;
    Some(date.with_hms(0, 0, 0).ok()?.assume_utc().unix_timestamp() * 1000)
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

#[cfg(test)]
mod tests {
    use super::MAX_CANONICAL_RATING;

    fn canonical_rating_distribution_density(ratings: impl IntoIterator<Item = f32>) -> Vec<f32> {
        const SAMPLE_STEP: f32 = 0.01;
        const BANDWIDTH: f32 = 0.10;

        let normalization = 1.0 / (BANDWIDTH * (2.0 * std::f32::consts::PI).sqrt());
        let ratings = ratings.into_iter().collect::<Vec<_>>();
        let samples = (MAX_CANONICAL_RATING / SAMPLE_STEP).round() as usize;

        if ratings.is_empty() {
            return vec![0.0; samples + 1];
        }

        (0..=samples)
            .map(|index| {
                let center = index as f32 * SAMPLE_STEP;
                ratings
                    .iter()
                    .map(|rating| {
                        let standardized = (center - *rating) / BANDWIDTH;
                        (-0.5 * standardized.powi(2)).exp() * normalization
                    })
                    .sum::<f32>()
                    / ratings.len() as f32
            })
            .collect()
    }

    #[test]
    fn canonical_distribution_density_peaks_near_the_rating() {
        let density = canonical_rating_distribution_density([2.0]);

        assert!(density[200] > density[180]);
        assert!(density[200] > density[220]);
        assert!((density[180] - density[220]).abs() < 0.0001);
    }

    #[test]
    fn canonical_distribution_density_shows_two_clusters_with_a_valley_between() {
        let density = canonical_rating_distribution_density([1.0, 1.0, 4.0, 4.0]);

        assert!(density[100] > density[250]);
        assert!(density[400] > density[250]);
        assert!((density[100] - density[400]).abs() < 0.0001);
    }
}
