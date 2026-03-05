use std::collections::HashMap;

use serde::Deserialize;
use time::UtcDateTime;

type RatingCategory = String;

pub type Data = HashMap<RatingCategory, Vec<Rating>>;
type Uri = String;

structstruck::strike!(
    #[structstruck::each[derive(Debug, Deserialize)]]
    #[structstruck::each[serde(rename_all = "camelCase")]]
    pub struct Rating {
        pub uid: String,
        pub added_at: String,
        pub added_by: struct {
            pub uri: Uri,
            pub username: String,
            pub display_name: String,
        },
        pub uri: Uri,
        pub name: String,
        pub album: struct {
            pub uri: Uri,
            pub name: String,
            pub artist: Artist,
            pub images: Vec<struct Image {
                pub url: String, // not an http(s) url
                pub label: String,
            }>,
        },
        pub artists: Vec<struct Artist {
            pub uri: Uri,
            pub name: String,
        }>,
        pub disc_number: u16,
        pub track_number: u16,
        pub duration: struct TrackDuration {
            pub milliseconds: u64,
        },
        pub is_explicit: bool,
        pub is_local: bool,
        pub is_playable: bool,
        pub is19_plus_only: bool,
        pub has_associated_video: bool,
        pub has_associated_audio: bool,
        pub is_banned: bool,
    }
);
