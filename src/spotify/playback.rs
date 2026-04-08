use dioxus::prelude::*;
#[cfg(feature = "server")]
use rspotify_model::PlayHistory;
use rspotify_model::{Context, CurrentPlaybackContext, PlaylistId, TrackId, Type};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::Duration;
use time::UtcDateTime;
#[cfg(test)]
use time::{Date, Time};

#[cfg(feature = "server")]
use crate::spotify::analyze::Analyzation;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PlaybackOptions {
    pub weighted_playback_playlists: HashSet<PlaylistId<'static>>,
}

impl PlaybackOptions {
    pub fn weighted_playback_enabled(&self, playlist: &PlaylistId<'static>) -> bool {
        self.weighted_playback_playlists.contains(playlist)
    }
}

#[cfg(feature = "server")]
pub async fn handle_weighted_playback() -> ! {
    let mut last_queued = None;
    loop {
        queue_random_song(&mut last_queued).await;

        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

// TODO:
// - only add a song to queue if it is empty
#[cfg(feature = "server")]
async fn queue_random_song(last_queued: &mut Option<(TrackId<'static>, usize)>) {
    use crate::spotify::recently_played_server;
    use crate::spotify::{
        add_to_queue, playback_options_server, playback_state_server, playlist_tracks_server,
        queue_server, ratings_server, saved_tracks_server, spotify,
    };

    use rspotify_model::{FullTrack, PlayableItem};

    let _spotify = spotify().await.clone();

    let queue = queue_server().await;
    let num_in_queue = |track_id| {
        queue
            .iter()
            .filter(|item| {
                if let PlayableItem::Track(FullTrack { id: Some(id), .. }) = item
                    && *id == track_id
                {
                    true
                } else {
                    false
                }
            })
            .count()
    };

    // only queue a song if the last one is no longer in the queue
    if let Some((track_id, times)) = last_queued {
        // we have to guard against the song already having been in the queue before us queuing
        if num_in_queue(track_id.clone()) >= *times {
            return;
        }
    }

    let context = playback_state_server().await;

    if let Some(CurrentPlaybackContext {
        context: Some(Context { _type, uri, .. }),
        ..
    }) = context
    {
        let tracks: Option<Vec<TrackId<'static>>> = match _type {
            Type::Playlist => match PlaylistId::from_id(&uri) {
                Ok(id) => {
                    let id = id.into_static();
                    if playback_options_server()
                        .await
                        .weighted_playback_enabled(&id)
                    {
                        Some(
                            playlist_tracks_server(id)
                                .await
                                .iter()
                                .filter_map(|track| track.id.clone().map(TrackId::into_static))
                                .collect::<Vec<_>>(),
                        )
                    } else {
                        None
                    }
                }
                Err(e) => {
                    warn!("_type = playlist uri ({uri}) should be a playlist id: {e}");
                    None
                }
            },
            Type::Collection => Some(saved_tracks_server().await.into_iter().collect::<Vec<_>>()),
            _ => None,
        };

        if let Some(tracks) = tracks {
            let ratings = ratings_server().await;
            let recently_played = recently_played_server().await;
            let track = choose_random_song(&tracks, &ratings, &recently_played);

            if let Some(track) = track {
                let res = add_to_queue(track.clone()).await;
                if let Err(e) = res {
                    warn!("{e}")
                } else {
                    *last_queued = Some((track.clone(), num_in_queue(track) + 1))
                }
            }
        }
    }
}

#[cfg(feature = "server")]
fn choose_random_song(
    tracks: &[TrackId<'static>],
    ratings: &Analyzation,
    recently_played: &[PlayHistory],
) -> Option<TrackId<'static>> {
    use rand::{RngExt, rng};

    let now = UtcDateTime::now();
    let weights = tracks.iter().map(|track| {
        let recently_played_multiplier = recently_played
            .iter()
            .find(|recent_track| recent_track.track.id == Some(track.clone()))
            .map(|recent_track| {
                weight_decay(
                    now,
                    UtcDateTime::from_unix_timestamp(recent_track.played_at.timestamp()).unwrap(),
                )
            })
            .unwrap_or(1.);
        weight(ratings.rating(track.as_ref())) * recently_played_multiplier
    });

    let total_weight: f32 = weights.clone().sum();

    let mut value = rng().random::<f32>() * total_weight;
    tracks
        .iter()
        .zip(weights)
        .find(|(_, weight)| {
            value -= weight;
            value <= 0.
        })
        .map(|(track, _)| track.clone())
}

pub fn weight_decay(now: UtcDateTime, then: UtcDateTime) -> f32 {
    let delta_minutes = (now - then).whole_minutes();
    1. - (1.6 * f32::exp(-0.05 * delta_minutes as f32)).clamp(0., 1.)
}

#[cfg(test)]
#[test]
fn test_weight_decay() {
    let now = UtcDateTime::new(
        Date::from_calendar_date(2000, time::Month::August, 23).unwrap(),
        Time::from_hms(8, 50, 0).unwrap(),
    );
    let offsets = [1, 5, 10, 15, 20, 30, 40, 50, 8 * 60];
    let expected = [
        0.0, 0.0, 0.02955091, 0.24421352, 0.41139287, 0.6429917, 0.78346354, 0.868664, 1.0,
    ];
    for (offset_mins, expected) in offsets.iter().zip(expected) {
        use time::Duration;

        let then = now - Duration::minutes(*offset_mins);
        let weight = weight_decay(now, then);
        assert_eq!(weight, expected)
    }
}

// TODO: add more parameters, i.e. playlist membership etc.
pub fn weight(rating: f32) -> f32 {
    2f32.powf(rating)
}
