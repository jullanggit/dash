use std::collections::HashMap;

use time::{Duration, UtcDateTime, format_description::well_known::Rfc3339};

use crate::ratings::types::Data;

pub struct AnalyzedData {
    pub songs: HashMap<Song, AnalyzedSong>,
}

#[derive(PartialEq, Eq, Hash)]
pub struct Song {
    pub name: String,
    pub artists: Vec<String>,
}

#[derive(Default)]
pub struct AnalyzedSong {
    pub canonical_rating: f32,
    pub rating_history: Vec<(f32, UtcDateTime)>,
    pub album: String,
    pub duration: Duration,
}

impl Data {
    pub fn analyze(&self) -> AnalyzedData {
        let mut out = AnalyzedData {
            songs: HashMap::new(),
        };

        for (rating_category, ratings) in self.iter() {
            let rating_category: f32 = rating_category.parse().unwrap();
            for rating in ratings {
                let song = Song {
                    name: rating.name.clone(),
                    artists: rating
                        .artists
                        .iter()
                        .map(|artist| artist.name.clone())
                        .collect(),
                };

                let entry = out.songs.entry(song).or_insert_with(AnalyzedSong::default);

                entry.rating_history.push((
                    rating_category,
                    UtcDateTime::parse(&rating.added_at, &Rfc3339).unwrap(),
                ));
                entry.album = rating.album.name.clone();
                entry.duration = Duration::milliseconds(rating.duration.milliseconds as i64);
            }
        }
        for (_, analyzed) in &mut out.songs {
            analyzed
                .rating_history
                .sort_by_key(|(_, date_time)| *date_time);
            analyzed.canonical_rating = canonical_rating(&analyzed.rating_history)
        }

        out
    }
}

pub fn canonical_rating(rating_history: &[(f32, UtcDateTime)]) -> f32 {
    const HALF_LIFE: Duration = Duration::weeks(26);

    let now = UtcDateTime::now();
    let (weighted_sum, weight_sum) =
        rating_history
            .iter()
            .fold((0., 0.), |(weighted_sum, weight_sum), (rating, time)| {
                let delta = now - *time;
                let weight = 0.5_f64.powf(delta / HALF_LIFE) as f32;

                (weighted_sum + *rating * weight, weight_sum + weight)
            });

    weighted_sum / weight_sum
}
