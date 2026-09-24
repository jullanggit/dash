//! rating log

use time::UtcDateTime;

use crate::spotify::analyze::TrackKey;

struct RatingLog {
    entries: Vec<Rating>,
}

struct Rating {
    track: TrackKey,
    value: f32,
    timestamp: UtcDateTime,
}
