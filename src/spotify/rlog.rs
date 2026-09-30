//! rating log

use std::{collections::HashMap, env::home_dir, sync::LazyLock};

use dioxus::prelude::{info, warn};
use persister::{Codec, Persister};
use rspotify_model::{FullTrack, SimplifiedArtist};
use serde::{Deserialize, Serialize};
use time::UtcDateTime;
#[cfg(feature = "server")]
use tokio::sync::Mutex;

use crate::spotify::analyze::{Analyzation, TrackAnalyzation, TrackKey};

#[cfg(feature = "server")]
pub static RLOG: LazyLock<Mutex<RatingLog>> =
    LazyLock::new(|| Mutex::new(RatingLog::open().expect("opening rating log should succeed")));

pub struct RatingLog {
    pub entries: Persister<Vec<Rating>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Rating {
    pub track: TrackKey,
    pub value: f32,
    pub timestamp: UtcDateTime,
}

impl RatingLog {
    #[cfg(feature = "server")]
    fn open() -> anyhow::Result<Self> {
        let path = home_dir()
            .ok_or(anyhow::anyhow!("Failed to get home dir"))?
            .join(".local/share/dash/rating-log.json");
        std::fs::create_dir_all(path.parent().unwrap())?;
        Ok(Self {
            entries: Persister::open_with(path)
                .codec(Codec::Json)
                .auto_save(true)
                .create_default()?,
        })
    }

    /// Sync between the rating log and spotify ratings (in-memory, doesn't add any tracks to actual playlists)
    pub async fn sync(
        &mut self,
        spotify_ratings: &mut HashMap<TrackKey, (FullTrack, TrackAnalyzation)>,
    ) -> persister::Result<()> {
        let spotify_log = spotify_ratings
            .iter()
            .flat_map(|(track, (_full_track, analyzation))| {
                analyzation
                    .rating_history
                    .iter()
                    .map(move |&(timestamp, value)| Rating {
                        track: track.clone(),
                        value,
                        timestamp: timestamp.clone(),
                    })
            });
        spotify_log.collect_into(&mut (*self.entries));

        self.entries
            .sort_unstable_by_key(|Rating { timestamp, .. }| *timestamp);
        self.entries.dedup();

        self.entries.save()?;

        for (_, (_, track_analyzation)) in spotify_ratings.iter_mut() {
            track_analyzation.rating_history.clear();
        }

        for Rating {
            track,
            value,
            timestamp,
        } in self.entries.iter()
        {
            let entry = spotify_ratings.entry(track.clone()).or_insert_with(|| {
                warn!("missing fulltrack for {track} while syncing tracks");
                (
                    make_dummy_fulltrack(track.clone()),
                    TrackAnalyzation::default(),
                )
            });

            entry.1.rating_history.push((*timestamp, *value));
        }

        Ok(())
    }
}

fn make_dummy_fulltrack(track: TrackKey) -> FullTrack {
    FullTrack {
        id: None,
        name: track.name,
        artists: track
            .artists
            .clone()
            .into_iter()
            .map(|artist| SimplifiedArtist {
                name: artist,
                external_urls: HashMap::new(),
                href: None,
                id: None,
            })
            .collect(),
        album: rspotify_model::SimplifiedAlbum {
            album_group: None,
            album_type: None,
            artists: Vec::new(),
            available_markets: Vec::new(),
            external_urls: HashMap::new(),
            href: None,
            id: None,
            images: Vec::new(),
            name: String::new(),
            release_date: None,
            release_date_precision: None,
            restrictions: None,
        },
        available_markets: Vec::new(),
        disc_number: 0,
        duration: Default::default(),
        explicit: false,
        external_ids: HashMap::new(),
        external_urls: HashMap::new(),
        href: None,
        is_local: false,
        is_playable: None,
        linked_from: None,
        restrictions: None,
        popularity: 0,
        preview_url: None,
        track_number: 0,
        r#type: rspotify_model::Type::Track,
    }
}
