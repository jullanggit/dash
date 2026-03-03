use std::collections::HashMap;

use serde::Deserialize;
use time::UtcDateTime;

type RatingCategory = String;

pub type Data = HashMap<RatingCategory, Vec<Rating>>;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rating {
    pub uid: String,
    pub added_at: String,
    pub added_by: AddedBy,
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
pub struct AddedBy {
    pub uri: Uri,
    pub username: String,
    pub display_name: String,
}

#[derive(Deserialize)]
pub struct Album {
    pub uri: Uri,
    pub name: String,
    pub artist: Artist,
    pub images: Vec<Image>,
}

#[derive(Deserialize)]
pub struct Image {
    pub url: String, // not an http(s) url
    pub label: String,
}

#[derive(Deserialize)]
pub struct Artist {
    pub uri: Uri,
    pub name: String,
}

#[derive(Deserialize)]
pub struct TrackDuration {
    pub milliseconds: u64,
}
