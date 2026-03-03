use std::collections::HashMap;

use serde::Deserialize;
use time::UtcDateTime;

type RatingCategory = String;
pub struct Data {
    pub ratings: HashMap<RatingCategory, Vec<Rating>>,
}

type Something = ();

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Rating {
    pub uid: String,
    pub play_index: Option<Something>,
    pub added_at: UtcDateTime,
    pub added_by: AddedBy,
    pub format_list_attributes: Something,
    pub uri: Uri,
    pub name: String,
    pub album: Album,
    pub artists: Vec<Artist>,
    pub disc_number: u16,
    pub track_number: u16,
    pub duration: TrackDuration,
    pub is_explicit: bool,
    pub is_local: bool,
    pub is_playable: bool,
    pub is19_plus_only: bool,
    pub has_associated_video: bool,
    pub has_associated_audio: bool,
    pub is_banned: bool,
}

type Uri = String;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddedBy {
    pub uri: Uri,
    pub username: String,
    pub display_name: String,
    pub images: Vec<Something>,
}

#[derive(Deserialize)]
struct Album {
    pub uri: Uri,
    pub name: String,
    pub artist: Artist,
    pub images: Vec<Image>,
}

#[derive(Deserialize)]
struct Image {
    pub url: String, // not an http(s) url
    pub label: String,
}

#[derive(Deserialize)]
struct Artist {
    pub uri: Uri,
    pub name: String,
}

#[derive(Deserialize)]
struct TrackDuration {
    pub milliseconds: u64,
}
