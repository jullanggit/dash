use std::env::home_dir;

use anyhow::Context;
use persister::{Codec, Persister};
use rspotify_model::{CurrentPlaybackContext, PlayableItem};
use serde::{Deserialize, Serialize};
use time::UtcDateTime;
use tokio::time::sleep;

use crate::spotify::{PLAYBACK_STATE_INTERVAL, analyze::TrackKey, playback_state_server};

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
                .create_default()?,
        };
        out.sort();

        Ok(out)
    }
    fn save(&mut self) -> anyhow::Result<()> {
        self.sort();
        self.entries.save().context("saving scrobble log")
    }
    fn sort(&mut self) {
        self.entries.sort_by_key(|scrobble| scrobble.start);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Scrobble {
    track: TrackKey,
    start: UtcDateTime,
    duration: time::Duration,
}

pub async fn scrobble() -> anyhow::Result<()> {
    let mut slog = ScrobbleLog::open()?;

    let mut last_persist = UtcDateTime::now();
    let mut last_track = None;
    loop {
        if let Some(CurrentPlaybackContext {
            is_playing: true,
            item: Some(PlayableItem::Track(ref fulltrack)),
            ..
        }) = playback_state_server().await.value
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
