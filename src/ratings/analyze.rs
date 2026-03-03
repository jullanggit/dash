use std::collections::HashMap;

use time::{Duration, UtcDateTime};

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

pub fn analyze(data: Data) -> AnalyzedData {
    let mut out = AnalyzedData {
        songs: HashMap::new(),
    };

    for (rating_category, ratings) in data.ratings {
        let rating_category: f32 = rating_category.parse().unwrap();
        for rating in ratings {
            let song = Song {
                name: rating.name,
                artists: rating
                    .artists
                    .iter()
                    .map(|artist| artist.name.clone())
                    .collect(),
            };

            let entry = out.songs.entry(song).or_insert_with(AnalyzedSong::default);

            entry
                .rating_history
                .push((rating_category, rating.added_at));
            entry.album = rating.album.name;
            entry.duration = Duration::milliseconds(rating.duration.milliseconds as i64);
        }
    }

    for (song, analyzed) in &mut out.songs {
        const HALF_LIFE: Duration = Duration::weeks(26);

        let now = UtcDateTime::now();
        let (weighted_sum, weight_sum) = analyzed.rating_history.iter().fold(
            (0., 0.),
            |(weighted_sum, weight_sum), (rating, time)| {
                let delta = now - *time;
                let weight = 0.5_f64.powf(dbg!(delta / HALF_LIFE)) as f32;

                (weighted_sum + *rating * weight, weight_sum + weight)
            },
        );

        analyzed.canonical_rating = weighted_sum / weight_sum;
    }

    out
}
