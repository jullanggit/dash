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
#[cfg(test)]
use crate::spotify::analyze::DEFAULT_RATING;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PlaybackSelection {
    #[default]
    Everything,
    RatedOnly,
    UnratedOnly,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaybackOptions {
    pub weighted_playback_playlists: HashSet<PlaylistId<'static>>,
    pub selection: PlaybackSelection,
    pub rating_cutoff: f32,
}

impl Default for PlaybackOptions {
    fn default() -> Self {
        Self {
            weighted_playback_playlists: HashSet::new(),
            selection: PlaybackSelection::Everything,
            rating_cutoff: 0.0,
        }
    }
}

impl PlaybackOptions {
    pub fn weighted_playback_enabled(&self, playlist: &PlaylistId<'static>) -> bool {
        self.weighted_playback_playlists.contains(playlist)
    }
}

impl PlaybackSelection {
    pub const ALL: [Self; 3] = [Self::Everything, Self::RatedOnly, Self::UnratedOnly];

    pub fn value(self) -> &'static str {
        match self {
            Self::Everything => "everything",
            Self::RatedOnly => "rated-only",
            Self::UnratedOnly => "unrated-only",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Everything => "Everything",
            Self::RatedOnly => "Rated Only",
            Self::UnratedOnly => "Unrated Only",
        }
    }

    pub fn from_value(value: &str) -> Option<Self> {
        Some(match value {
            "everything" => Self::Everything,
            "rated-only" => Self::RatedOnly,
            "unrated-only" => Self::UnratedOnly,
            _ => return None,
        })
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
            let options = playback_options_server().await;
            let track = choose_random_song(
                &tracks,
                &ratings,
                &recently_played,
                options.selection,
                options.rating_cutoff,
            );

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
    selection: PlaybackSelection,
    rating_cutoff: f32,
) -> Option<TrackId<'static>> {
    use rand::{RngExt, rng};

    let tracks = tracks
        .iter()
        .filter(|track| {
            let is_rated = ratings
                .tracks
                .iter()
                .any(|(rated_track, _)| rated_track.id.as_ref() == Some(track));
            let rating = ratings.rating(track.as_ref().clone());
            if rating < rating_cutoff {
                return false;
            }
            match selection {
                PlaybackSelection::Everything => true,
                PlaybackSelection::RatedOnly => is_rated,
                PlaybackSelection::UnratedOnly => !is_rated,
            }
        })
        .collect::<Vec<_>>();
    if tracks.is_empty() {
        return None;
    }

    let now = UtcDateTime::now();
    let weights = tracks.iter().map(|track| {
        let recently_played_multiplier = recently_played
            .iter()
            .find(|recent_track| recent_track.track.id.as_ref() == Some(*track))
            .map(|recent_track| {
                weight_decay(
                    now,
                    UtcDateTime::from_unix_timestamp(recent_track.played_at.timestamp()).unwrap(),
                )
            })
            .unwrap_or(1.);
        weight(ratings.rating(track.as_ref().clone())) * recently_played_multiplier
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
        .map(|(track, _)| (*track).clone())
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

#[cfg(test)]
mod tests {
    use super::{PlaybackSelection, choose_random_song};
    use crate::spotify::analyze::{Analyzation, TrackAnalyzation};
    use rspotify_model::{FullTrack, Id, PlayHistory, TrackId};

    fn track(id: &'static str) -> TrackId<'static> {
        TrackId::from_id(id).unwrap().into_static()
    }

    #[test]
    fn choose_random_song_filters_to_rated_tracks() {
        let rated = track("4uLU6hMCjMI75M1A2tKUQC");
        let unrated = track("1301WleyT98MSxVHPZCA6M");
        let tracks = [rated.clone(), unrated];
        let ratings = Analyzation {
            tracks: vec![(
                FullTrack {
                    id: Some(rated.clone()),
                    ..FullTrack::default()
                },
                TrackAnalyzation {
                    canonical_rating: 4.0,
                    ..TrackAnalyzation::default()
                },
            )],
            ..Analyzation::default()
        };

        let selected =
            choose_random_song(&tracks, &ratings, &[], PlaybackSelection::RatedOnly, 0.0);

        assert_eq!(selected, Some(rated));
    }

    #[test]
    fn choose_random_song_returns_none_when_filter_excludes_everything() {
        let unrated = track("1301WleyT98MSxVHPZCA6M");
        let tracks = [unrated];

        let selected = choose_random_song(
            &tracks,
            &Analyzation::default(),
            &[] as &[PlayHistory],
            PlaybackSelection::RatedOnly,
            0.0,
        );

        assert_eq!(selected, None);
    }

    #[test]
    fn choose_random_song_filters_to_unrated_tracks() {
        let rated = track("4uLU6hMCjMI75M1A2tKUQC");
        let unrated = track("1301WleyT98MSxVHPZCA6M");
        let tracks = [rated.clone(), unrated.clone()];
        let ratings = Analyzation {
            tracks: vec![(
                FullTrack {
                    id: Some(rated),
                    ..FullTrack::default()
                },
                TrackAnalyzation {
                    canonical_rating: 4.0,
                    ..TrackAnalyzation::default()
                },
            )],
            ..Analyzation::default()
        };

        let selected = choose_random_song(
            &tracks,
            &ratings,
            &[] as &[PlayHistory],
            PlaybackSelection::UnratedOnly,
            0.0,
        );

        assert_eq!(selected, Some(unrated));
    }

    #[test]
    fn choose_random_song_applies_cutoff_to_default_rating_for_unrated_tracks() {
        let unrated = track("1301WleyT98MSxVHPZCA6M");
        let tracks = [unrated];

        let selected = choose_random_song(
            &tracks,
            &Analyzation::default(),
            &[] as &[PlayHistory],
            PlaybackSelection::Everything,
            DEFAULT_RATING + 0.1,
        );

        assert_eq!(selected, None);
    }
}
