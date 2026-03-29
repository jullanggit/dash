use dioxus::prelude::*;
use rspotify_model::{FullTrack, PlaylistId, TrackId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use time::{Date, Duration, UtcDateTime};

// TODO: make this configurable
pub const DEFAULT_RATING: f32 = 2.5;
const RATING_OVERWRITE_WINDOW: Duration = Duration::minutes(5);

/// Contains all analyzations derived from `rating_history` and the providing track
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct TrackAnalyzation {
    /// sorted by ascending date
    pub rating_history: Vec<(UtcDateTime, f32)>,
    pub canonical_rating_history: Vec<(UtcDateTime, f32)>,
    pub canonical_rating: f32,
    pub genres: HashSet<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Analyzation {
    pub tracks: AnalyzedTracks,
    /// sorted by ascending date
    pub average_rating_per_day: Vec<(Date, f32)>,
    pub num_ratings_history: Vec<(UtcDateTime, u32)>,
    pub num_rated_tracks_history: Vec<(UtcDateTime, u32)>,
    #[serde(default)]
    pub playlist_snapshot_ids: HashMap<PlaylistId<'static>, String>,
}
impl Analyzation {
    pub fn rating(&self, track_id: TrackId<'_>) -> f32 {
        self.tracks
            .iter()
            .find(|(track, _)| track.id.as_ref() == Some(&track_id))
            .map(|(_, analyzation)| analyzation.canonical_rating)
            .unwrap_or(DEFAULT_RATING)
    }
}

pub type AnalyzedTracks = Vec<(FullTrack, TrackAnalyzation)>;

fn dedupe_rating_history(rating_history: &mut Vec<(UtcDateTime, f32)>) {
    rating_history.sort_unstable_by_key(|&(time, _)| time);

    let mut deduped = Vec::with_capacity(rating_history.len());
    for (time, rating) in rating_history.drain(..) {
        if let Some((previous_time, previous_rating)) = deduped.last_mut()
            && time - *previous_time <= RATING_OVERWRITE_WINDOW
        {
            *previous_time = time;
            *previous_rating = rating;
        } else {
            deduped.push((time, rating));
        }
    }

    *rating_history = deduped;
}

/// Build analyzation based on tracks and rating histories
#[cfg(feature = "server")]
pub async fn analyze(mut tracks: AnalyzedTracks) -> Analyzation {
    use crate::spotify::genres;

    trace!("Analyzing ratings");

    fn canonical_rating(rating_history: impl IntoIterator<Item = (f32, UtcDateTime)>) -> f32 {
        const HALF_LIFE: Duration = Duration::weeks(26);

        let now = UtcDateTime::now();
        let (weighted_sum, weight_sum) = rating_history.into_iter().fold(
            (0., 0.),
            |(weighted_sum, weight_sum), (rating, time)| {
                let delta = now - time;
                let weight = 0.5_f64.powf(delta / HALF_LIFE) as f32;

                (weighted_sum + rating * weight, weight_sum + weight)
            },
        );

        weighted_sum / weight_sum
    }

    // track analyzations
    for i in 0..tracks.len() {
        let artists = {
            let (track, analyzation) = &mut tracks[i];

            dedupe_rating_history(&mut analyzation.rating_history);

            analyzation.canonical_rating_history = (1..=analyzation.rating_history.len())
                .map(|i| {
                    (
                        analyzation.rating_history[i - 1].0,
                        canonical_rating(
                            analyzation
                                .rating_history
                                .iter()
                                .take(i)
                                .map(|&(time, rating)| (rating, time)),
                        ),
                    )
                })
                .collect();
            analyzation.canonical_rating = analyzation
                .canonical_rating_history
                .last()
                .map(|(_, rating)| *rating)
                .unwrap_or(DEFAULT_RATING);

            track.artists.clone()
        };

        tracks[i].1.genres = genres(artists).await;
    }

    // cross-track analyzations
    let average_rating_per_day = {
        let ratings_per_day: BTreeMap<Date, Vec<f32>> = tracks
            .iter()
            .flat_map(|(_, track_analyzation)| track_analyzation.rating_history.iter())
            .fold(BTreeMap::new(), |mut acc, (date_time, rating)| {
                let date = date_time.date();
                acc.entry(date).or_default().push(*rating);
                acc
            });

        ratings_per_day
            .iter()
            .map(|(&date, ratings)| {
                let average_rating =
                    ratings.iter().map(f32::clone).sum::<f32>() / ratings.len() as f32;
                (date, average_rating)
            })
            .collect()
    };

    let (num_ratings_history, num_rated_tracks_history) = {
        let rating_times = tracks
            .iter()
            .flat_map(|(_, data)| data.rating_history.iter().map(|(time, _)| time))
            .collect::<Vec<_>>();

        let first_rating_times = tracks
            .iter()
            .filter_map(|(_, data)| data.rating_history.iter().map(|(time, _)| time).min())
            .collect::<Vec<_>>();

        let history = |mut times: Vec<&UtcDateTime>| {
            times.sort_unstable();
            times
                .iter()
                .enumerate()
                .map(|(count, &&date_time)| (date_time, count as u32))
                .collect()
        };

        (history(rating_times), history(first_rating_times))
    };

    Analyzation {
        tracks,
        average_rating_per_day,
        num_ratings_history,
        num_rated_tracks_history,
        playlist_snapshot_ids: HashMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::dedupe_rating_history;
    use time::{Date, Duration, Month, Time, UtcDateTime};

    fn date_time(minute: u8) -> UtcDateTime {
        UtcDateTime::new(
            Date::from_calendar_date(2024, Month::January, 1).unwrap(),
            Time::from_hms(0, minute, 0).unwrap(),
        )
    }

    #[test]
    fn ratings_within_five_minutes_overwrite_the_previous_rating() {
        let mut history = vec![
            (date_time(0), 2.0),
            (date_time(4), 4.0),
            (date_time(10), 3.0),
        ];

        dedupe_rating_history(&mut history);

        assert_eq!(history, vec![(date_time(4), 4.0), (date_time(10), 3.0)]);
    }

    #[test]
    fn ratings_more_than_five_minutes_apart_are_kept() {
        let mut history = vec![(date_time(0), 2.0), (date_time(6), 4.0)];

        dedupe_rating_history(&mut history);

        assert_eq!(history, vec![(date_time(0), 2.0), (date_time(6), 4.0)]);
    }

    #[test]
    fn overwrite_window_is_chained_from_the_latest_rating() {
        let mut history = vec![
            (date_time(0), 1.0),
            (date_time(4), 2.0),
            (date_time(8), 3.0),
            (date_time(14), 4.0),
        ];

        dedupe_rating_history(&mut history);

        assert_eq!(history, vec![(date_time(8), 3.0), (date_time(14), 4.0)]);
        assert_eq!(history[1].0 - history[0].0, Duration::minutes(6));
    }
}
