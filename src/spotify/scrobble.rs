use std::env::home_dir;

use anyhow::Context;
use dioxus::logger::tracing::warn;
use persister::{Codec, Persister};
use rspotify_model::{CurrentPlaybackContext, PlayableItem, TrackId};
use serde::{Deserialize, Serialize};
use time::{UtcDateTime, format_description::well_known::Rfc3339};
use tokio::{
    fs::{self, read_dir},
    sync::Mutex,
    time::sleep,
};

use crate::spotify::{
    PLAYBACK_STATE_INTERVAL,
    analyze::{PlayHistory, TrackKey},
    caching::WithLastFetched,
    full_track, playback_state_server, ratings_server,
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
    fn open() -> anyhow::Result<Self> {
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
}

/// Load scrobbles from Spotify's extended streaming history, if present.
/// `notes`: the returned entries are unsorted.
async fn spotify_extended_streaming_history() -> anyhow::Result<Vec<Scrobble>> {
    #[derive(Deserialize)]
    struct StreamingHistoryEntry {
        ts: String,
        ms_played: u32,
        spotify_track_uri: Option<String>,
    }
    let mut read_dir = read_dir("/tmp/Spotify Extended Streaming History").await?;
    let mut scrobbles = Vec::new();

    let ratings = &ratings_server().await.value;

    while let Ok(Some(entry)) = read_dir.next_entry().await {
        if entry.file_name().as_encoded_bytes().ends_with(b".json") {
            let data: Vec<StreamingHistoryEntry> =
                serde_json::from_slice(&fs::read(entry.path()).await?)?;

            for entry in data {
                if let Some(uri) = &entry.spotify_track_uri
                    && let Ok(track_id) = TrackId::from_uri(uri)
                {
                    let full_track = full_track(None, &track_id, ratings).await;
                    scrobbles.push(Scrobble {
                        track: TrackKey::from_track(&full_track),
                        start: UtcDateTime::parse(&entry.ts, &Rfc3339)
                            .expect("`ts` should be in rfc3339 format"),
                        duration: time::Duration::milliseconds(entry.ms_played.into()),
                    });
                }
            }
        }
    }

    Ok(scrobbles)
}

/// The shared, in-memory scrobble log, populated once [`scrobble`] opens it.
static SLOG: Mutex<Option<ScrobbleLog>> = Mutex::const_new(None);

/// Played time per track, derived from the in-memory scrobble log.
pub async fn play_history() -> std::sync::Arc<PlayHistory> {
    let slog = SLOG.lock().await;
    let Some(slog) = slog.as_ref() else {
        return std::sync::Arc::default();
    };

    let mut history = PlayHistory::new();
    for Scrobble {
        track,
        start,
        duration,
    } in slog.entries.iter()
    {
        history
            .entry(track.clone())
            .or_default()
            .push((*start, *duration));
    }
    for plays in history.values_mut() {
        plays.sort_unstable_by_key(|(start, _)| *start);
    }

    std::sync::Arc::new(history)
}

pub async fn scrobble() -> anyhow::Result<()> {
    // Publish the persisted log before loading the extended streaming history: loading
    // it may trigger an analyzation (via `ratings_server`) which reads the log through
    // `play_history`.
    let log = ScrobbleLog::open()?;
    *SLOG.lock().await = Some(log);

    match spotify_extended_streaming_history().await {
        Ok(entries) => {
            let mut slog = SLOG.lock().await;
            let slog = slog.as_mut().expect("scrobble log is initialized above");
            slog.entries.extend(entries);
            slog.sort();
            slog.dedup();
        }
        Err(e) => warn!("Failed to add spotify extended streaming history: {e:#}"),
    }

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

            let fetch_diff = *last_fetched - last_last_fetched;
            // guard against retry-afters while requesting playback state. After a long one, the same song _could_ still be playing, but what happened inbetween is unknown
            let duration_played = if fetch_diff > time::Duration::minutes(5) {
                time::Duration::seconds(1)
            } else if last_track.as_ref() == Some(&track) {
                fetch_diff
            } else {
                fetch_diff / 2 // wasn't playing before, take average
            };

            {
                let mut slog = SLOG.lock().await;
                let slog = slog.as_mut().expect("scrobble log is initialized above");
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

                if last_persist + tokio::time::Duration::from_mins(1) < now {
                    slog.save()?;
                    last_persist = now;
                }
            }

            last_track = Some(track);
            last_last_fetched = *last_fetched;
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
