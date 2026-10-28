use std::env::home_dir;

use anyhow::Context;
use dioxus::logger::tracing::warn;
use persister::{Codec, Persister};
use rspotify_model::{CurrentPlaybackContext, PlayableItem, TrackId};
use serde::{Deserialize, Serialize};
use time::{UtcDateTime, format_description::well_known::Rfc3339};
use tokio::{
    fs::{self, read_dir},
    time::sleep,
};

use crate::spotify::{
    PLAYBACK_STATE_INTERVAL, analyze::TrackKey, caching::WithLastFetched, full_track_maybe_cached,
    playback_state_server, ratings_server,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Scrobble {
    track: TrackKey,
    start: UtcDateTime,
    duration: time::Duration,
}

struct ScrobbleLog {
    /// sorted by start time
    pub entries: Persister<Vec<Scrobble>>,
}
impl ScrobbleLog {
    async fn open() -> anyhow::Result<Self> {
        let path = home_dir()
            .ok_or(anyhow::anyhow!("Failed to get home dir"))?
            .join(".local/share/dash/scrobbles.json");
        std::fs::create_dir_all(path.parent().unwrap())?;

        let mut out = Self {
            entries: Persister::open_with(path)
                .codec(Codec::Json)
                .auto_save(true)
                .open()?,
        };

        let _ = out.add_spotify_extended_streaming_history();

        out.sort();
        out.dedup();

        Ok(out)
    }
    fn save(&mut self) -> anyhow::Result<()> {
        self.sort();
        self.entries.save().context("saving scrobble log")
    }
    fn sort(&mut self) {
        self.entries.sort_by_key(|scrobble| scrobble.start);
    }
    /// collapse overlapping scrobbles.
    /// requires `self.entries` to be sorted.
    fn dedup(&mut self) {
        let mut i = 0;
        while i + 1 < self.entries.len() {
            let mut increment = true;

            let (a, b) = (&self.entries[i], &self.entries[i + 1]);

            if a.start + a.duration > b.start {
                if a.track == b.track {
                    let end = (a.start + a.duration).max(b.start + b.duration);

                    let a = &mut self.entries[i];
                    a.duration = end - a.start;

                    self.entries.remove(i + 1);
                    increment = false;
                } else {
                    warn!(
                        "Overlapping scrobbles of different songs; This is not handled and may prevent subsequent deduplication."
                    );
                }
            }

            if increment {
                i += 1;
            }
        }
    }
    /// leaves `self.entries` unsorted
    async fn add_spotify_extended_streaming_history(&mut self) -> anyhow::Result<()> {
        #[derive(Deserialize)]
        struct StreamingHistoryEntry {
            ts: String,
            ms_played: u32,
            spotify_track_uri: String,
        }
        let mut read_dir = read_dir("/tmp/Spotify Extended Streaming History").await?;

        let ratings = &ratings_server().await.value;

        while let Ok(Some(entry)) = read_dir.next_entry().await {
            if entry.file_name().as_encoded_bytes().ends_with(b".json") {
                let data: Vec<StreamingHistoryEntry> =
                    serde_json::from_slice(&fs::read(entry.path()).await?)?;

                for entry in data {
                    if let Ok(track_id) = TrackId::from_uri(&entry.spotify_track_uri)
                        && let Ok(full_track) =
                            full_track_maybe_cached(None, &track_id, ratings).await
                    {
                        self.entries.push(Scrobble {
                            track: TrackKey::from_track(&full_track),
                            start: UtcDateTime::parse(&entry.ts, &Rfc3339)
                                .expect("`ts` should be in rfc3339 format"),
                            duration: time::Duration::milliseconds(entry.ms_played.into()),
                        });
                    }
                }
            }
        }

        Ok(())
    }
}

pub async fn scrobble() -> anyhow::Result<()> {
    let mut slog = ScrobbleLog::open().await?;

    let mut last_persist = UtcDateTime::now();
    let mut last_last_fetched = UtcDateTime::MIN;
    let mut last_track = None;
    loop {
        if let WithLastFetched { last_fetched, value: Some(CurrentPlaybackContext {
            is_playing: true,
            item: Some(PlayableItem::Track(fulltrack)),
            ..
        })} = playback_state_server().await.as_ref()
            // guard against stale cache
            && *last_fetched > last_last_fetched
        {
            let now = UtcDateTime::now();
            let track = TrackKey::from_track(fulltrack);

            let duration_played = if last_track.as_ref() == Some(&track) {
                PLAYBACK_STATE_INTERVAL
            } else {
                PLAYBACK_STATE_INTERVAL / 2 // wasn't playing before, take average
            };
            if let Some(last) = slog.entries.last_mut()
                && last.track == track
            {
                last.duration += duration_played;
            } else {
                slog.entries.push(Scrobble {
                    track: track.clone(),
                    start: now - duration_played,
                    duration: duration_played,
                });
            }

            last_track = Some(track);
            last_last_fetched = *last_fetched;

            if last_persist + tokio::time::Duration::from_mins(1) < now {
                slog.save()?;
                last_persist = now;
            }
        } else {
            last_track = None;
        }

        sleep(
            PLAYBACK_STATE_INTERVAL
                .try_into()
                .expect("Playback state refresh interval should be positive"),
        )
        .await;
    }
}

#[cfg(test)]
mod test {
    use time::{Date, Duration, Month, Time};

    use super::*;

    #[test]
    fn test_dedup() {
        let mut test = ScrobbleLog {
            entries: Persister::init(
                "target/test/test.json",
                vec![
                    Scrobble {
                        start: UtcDateTime::new(
                            Date::from_calendar_date(2026, Month::July, 2).unwrap(),
                            Time::MIDNIGHT,
                        ),
                        duration: Duration::minutes(1),
                        track: TrackKey::default(),
                    },
                    Scrobble {
                        start: UtcDateTime::new(
                            Date::from_calendar_date(2026, Month::July, 1).unwrap(),
                            Time::MIDNIGHT,
                        ),
                        duration: Duration::minutes(3),
                        track: TrackKey::default(),
                    },
                    Scrobble {
                        start: UtcDateTime::new(
                            Date::from_calendar_date(2026, Month::July, 3).unwrap(),
                            Time::MIDNIGHT,
                        ),
                        duration: Duration::minutes(2),
                        track: TrackKey {
                            name: "a".into(),
                            artists: Vec::new(),
                        },
                    },
                    Scrobble {
                        start: UtcDateTime::new(
                            Date::from_calendar_date(2026, Month::July, 1).unwrap(),
                            Time::MIDNIGHT + Duration::minutes(2),
                        ),
                        duration: Duration::minutes(2),
                        track: TrackKey::default(),
                    },
                ],
            )
            .unwrap(),
        };

        test.sort();
        test.dedup();

        assert_eq!(
            test.entries.as_slice(),
            &[
                Scrobble {
                    start: UtcDateTime::new(
                        Date::from_calendar_date(2026, Month::July, 1).unwrap(),
                        Time::MIDNIGHT,
                    ),
                    duration: Duration::minutes(4),
                    track: TrackKey::default(),
                },
                Scrobble {
                    start: UtcDateTime::new(
                        Date::from_calendar_date(2026, Month::July, 2).unwrap(),
                        Time::MIDNIGHT,
                    ),
                    duration: Duration::minutes(1),
                    track: TrackKey::default(),
                },
                Scrobble {
                    start: UtcDateTime::new(
                        Date::from_calendar_date(2026, Month::July, 3).unwrap(),
                        Time::MIDNIGHT,
                    ),
                    duration: Duration::minutes(2),
                    track: TrackKey {
                        name: "a".into(),
                        artists: Vec::new(),
                    },
                },
            ]
        )
    }
}
