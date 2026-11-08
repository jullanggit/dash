use dioxus::prelude::*;
use rspotify_model::{FullTrack, PlaylistId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use time::{Date, Duration, UtcDateTime};

pub const RATING_OVERWRITE_WINDOW: Duration = Duration::minutes(5);

/// A stable key for identifying a track by its name and sorted artist names.
/// This is preferred over Spotify's `TrackId` because it's invariant across
/// re-releases, remasters, and different Spotify catalog versions of the same song.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct TrackKey {
    pub name: String,
    /// sorted alphabetically for consistent comparison
    pub artists: Vec<String>,
}

/// Serialize as a JSON string so it can be used as a `HashMap` key with `serde_json`.
impl Serialize for TrackKey {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::Error;
        let inner = serde_json::json!([self.name, self.artists]);
        let repr = serde_json::to_string(&inner)
            .map_err(|e| Error::custom(format!("failed to serialize TrackKey: {e:#}")))?;
        serializer.serialize_str(&repr)
    }
}
impl<'de> Deserialize<'de> for TrackKey {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let repr: String = Deserialize::deserialize(deserializer)?;
        let arr: serde_json::Value = serde_json::from_str(&repr)
            .map_err(|e| Error::custom(format!("failed to deserialize TrackKey: {e:#}")))?;
        let name = arr[0]
            .as_str()
            .ok_or_else(|| Error::custom("TrackKey: missing name"))?
            .to_string();
        let artists = arr[1]
            .as_array()
            .ok_or_else(|| Error::custom("TrackKey: missing artists"))?
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
        Ok(TrackKey { name, artists })
    }
}

impl TrackKey {
    pub fn from_track(track: &FullTrack) -> Self {
        let mut artists: Vec<String> = track.artists.iter().map(|a| a.name.clone()).collect();
        artists.sort();
        Self {
            name: track.name.clone(),
            artists,
        }
    }
}

impl std::fmt::Display for TrackKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} by {}", self.name, self.artists.join(", "))
    }
}

/// Contains all analyzations derived from `rating_history` and the providing track
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct TrackAnalyzation {
    /// sorted by ascending date
    pub rating_history: Vec<(UtcDateTime, f32)>,
    pub canonical_rating_history: Vec<(UtcDateTime, f32)>,
    pub canonical_rating: f32,
    /// genre name -> weight
    pub genres: HashMap<String, f32>,
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
    #[serde(default)]
    pub last_full_refetch: HashMap<PlaylistId<'static>, UtcDateTime>,
}
impl Analyzation {
    pub fn rating(&self, key: &TrackKey) -> Option<f32> {
        self.tracks
            .get(key)
            .map(|(_, analyzation)| analyzation.canonical_rating)
    }

    pub fn contains(&self, key: &TrackKey) -> bool {
        self.tracks.contains_key(key)
    }
}

pub type AnalyzedTracks = HashMap<TrackKey, (FullTrack, TrackAnalyzation)>;

/// All plays of a track as `(start, played duration)`, sorted by start time.
#[cfg(feature = "server")]
pub type PlayHistory = HashMap<TrackKey, Vec<(UtcDateTime, Duration)>>;

/// Half life of a rating in full plays
#[cfg(feature = "server")]
const PLAYS_HALF_LIFE: f64 = 15.0;

/// Familiarity with a track based on the number of full plays
#[cfg(feature = "server")]
fn familiarity_weight(full_plays: f64) -> f64 {
    1.0 + full_plays.ln_1p()
}

/// Relative weight of a rating after `plays_since` full plays have happened.
#[cfg(feature = "server")]
fn plays_decay(plays_since: f64) -> f64 {
    0.5_f64.powf(plays_since / PLAYS_HALF_LIFE)
}

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

/// Build analyzation based on tracks, rating histories and play histories
#[cfg(feature = "server")]
pub async fn analyze(mut tracks: AnalyzedTracks, play_history: &PlayHistory) -> Analyzation {
    use crate::spotify::{ArtistGenreCache, genres_cached};

    trace!("Analyzing ratings");

    // Genre lookups depend only on the artist, so share one cache across the whole pass.
    let mut artist_genre_cache = ArtistGenreCache::new();

    // track analyzations
    for (key, (track, analyzation)) in tracks.iter_mut() {
        dedupe_rating_history(&mut analyzation.rating_history);

        if analyzation.rating_history.is_empty() {
            continue;
        }

        // Familiarity is measured in full plays: total played time / track duration.
        let track_duration_secs = track.duration.num_milliseconds() as f64 / 1000.0;
        let track_duration_secs = if track_duration_secs > 0.0 {
            track_duration_secs
        } else {
            180.0 // default to three minutes
        };
        let plays = play_history.get(key).map(Vec::as_slice).unwrap_or_default();
        let full_plays = |played_secs: f64| played_secs / track_duration_secs;

        let last_rating_time = analyzation
            .rating_history
            .last()
            .map(|(time, _)| *time)
            .expect("checked non-empty above");
        let plays_at_last_rating = full_plays(
            plays
                .iter()
                .take_while(|(start, _)| *start <= last_rating_time)
                .map(|(_, duration)| duration.as_seconds_f64())
                .sum(),
        );

        let mut played_secs = 0.0;
        let mut next_play = 0;

        let mut weighted_sum = 0.0;
        let mut weight_sum = 0.0;
        let canonical_rating_history = analyzation
            .rating_history
            .iter()
            .map(|&(time, rating)| {
                // accumulate the plays that happened before this rating
                while let Some((start, duration)) = plays.get(next_play)
                    && *start <= time
                {
                    played_secs += duration.as_seconds_f64();
                    next_play += 1;
                }

                let plays_before = full_plays(played_secs);
                let weight = familiarity_weight(plays_before)
                    * plays_decay(plays_at_last_rating - plays_before);
                weighted_sum += rating as f64 * weight;
                weight_sum += weight;
                (time, (weighted_sum / weight_sum) as f32)
            })
            .collect::<Vec<_>>();

        analyzation.canonical_rating = canonical_rating_history
            .last()
            .map(|(_, rating)| *rating)
            .expect("a to-be-analyzed track should have a rating");
        analyzation.canonical_rating_history = canonical_rating_history;

        analyzation.genres = genres_cached(track, &mut artist_genre_cache).await;
    }

    // cross-track analyzations
    let average_rating_per_day = {
        let ratings_per_day: BTreeMap<Date, Vec<f32>> = tracks
            .iter()
            .flat_map(|(_, (_, analyzation))| analyzation.rating_history.iter())
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
            .flat_map(|(_, (_, data))| data.rating_history.iter().map(|(time, _)| time))
            .collect::<Vec<_>>();

        let first_rating_times = tracks
            .iter()
            .filter_map(|(_, (_, data))| data.rating_history.iter().map(|(time, _)| time).min())
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
        last_full_refetch: HashMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use crate::spotify::{
        analyze::{
            AnalyzedTracks, PLAYS_HALF_LIFE, PlayHistory, TrackAnalyzation, TrackKey, analyze,
            familiarity_weight, plays_decay,
        },
        rlog::make_dummy_fulltrack,
    };

    use super::dedupe_rating_history;
    use time::{Date, Duration, Month, Time, UtcDateTime};

    fn date_time(minute: u8) -> UtcDateTime {
        UtcDateTime::new(
            Date::from_calendar_date(2024, Month::January, 1).unwrap(),
            Time::from_hms(0, minute, 0).unwrap(),
        )
    }

    fn midnight(year: i32, month: Month, day: u8) -> UtcDateTime {
        UtcDateTime::new(
            Date::from_calendar_date(year, month, day).unwrap(),
            Time::MIDNIGHT,
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

    #[test]
    fn familiarity_grows_logarithmically() {
        assert_eq!(familiarity_weight(0.0), 1.0);

        // every additional play adds less confidence than the previous one
        let gains = (0..4)
            .map(|plays| {
                let plays = plays as f64;
                familiarity_weight(plays + 1.0) - familiarity_weight(plays)
            })
            .collect::<Vec<_>>();
        assert!(gains.windows(2).all(|pair| pair[1] < pair[0]), "{gains:?}");
    }

    #[test]
    fn rating_weight_halves_per_play_half_life() {
        assert!((plays_decay(0.0) - 1.0).abs() < 1e-12);
        assert!((plays_decay(PLAYS_HALF_LIFE) - 0.5).abs() < 1e-12);
        assert!((plays_decay(2.0 * PLAYS_HALF_LIFE) - 0.25).abs() < 1e-12);
    }

    #[test]
    fn ratings_are_weighted_by_full_plays() {
        let track_key = TrackKey::default();

        // `FullTrack::duration` is a `chrono::Duration`, which isn't a direct dependency
        // here, so set it through the model's JSON representation.
        let mut track_json = serde_json::to_value(make_dummy_fulltrack(track_key.clone())).unwrap();
        track_json["duration_ms"] = serde_json::json!(180_000);
        let track: rspotify_model::FullTrack = serde_json::from_value(track_json).unwrap();

        // A 3-minute song, so each play below is a full play.
        let play = |month, day| (midnight(2026, month, day), Duration::minutes(3));
        let play_history = PlayHistory::from([(
            track_key.clone(),
            vec![
                play(Month::June, 1),      // 1 play before the first rating
                play(Month::August, 1),    // 2 plays before the second rating
                play(Month::September, 1), // 4 plays before the third rating
                play(Month::September, 2),
            ],
        )]);

        let analyzation = tokio::runtime::Runtime::new().unwrap().block_on(analyze(
            AnalyzedTracks::from([(
                track_key,
                (
                    track,
                    TrackAnalyzation {
                        rating_history: vec![
                            (midnight(2026, Month::July, 1), 4.0),
                            (midnight(2026, Month::August, 20), 4.5),
                            (midnight(2026, Month::October, 18), 3.0),
                        ],
                        ..Default::default()
                    },
                ),
            )]),
            &play_history,
        ));

        assert_eq!(
            analyzation
                .tracks
                .into_values()
                .next()
                .unwrap()
                .1
                .canonical_rating_history
                .into_iter()
                .map(|(_, value)| (value * 100.) as u16)
                .collect::<Vec<_>>(),
            // play counts at the ratings are 1, 2 and 4; familiarity is log-scaled and the
            // ratings are 3, 2 and 0 full plays apart, so older ones decay by
            // 0.5^(plays_since / PLAYS_HALF_LIFE)
            vec![400, 428, 372]
        );
    }
    #[test]
    fn zero_duration_gets_defaulted() {
        let track_key = TrackKey::default();

        // default fulltrack, so zero duration
        let track: rspotify_model::FullTrack = make_dummy_fulltrack(track_key.clone());

        let play = |month, day, plays| {
            (
                midnight(2026, month, day),
                // default song duration is three minutes
                Duration::seconds((180. * plays) as i64),
            )
        };
        let play_history = PlayHistory::from([(
            track_key.clone(),
            vec![
                play(Month::October, 11, 1.8), // before first rating
                play(Month::November, 7, 6.),  // before second rating
            ],
        )]);

        let analyzation = tokio::runtime::Runtime::new().unwrap().block_on(analyze(
            AnalyzedTracks::from([(
                track_key,
                (
                    track,
                    TrackAnalyzation {
                        rating_history: vec![
                            (midnight(2026, Month::October, 12), 3.66),
                            (midnight(2026, Month::November, 8), 3.99),
                        ],
                        ..Default::default()
                    },
                ),
            )]),
            &play_history,
        ));

        assert_eq!(
            analyzation
                .tracks
                .into_values()
                .next()
                .unwrap()
                .1
                .canonical_rating_history
                .into_iter()
                .map(|(_, value)| (value * 100.) as u16)
                .collect::<Vec<_>>(),
            vec![366, 388]
        );
    }
}
