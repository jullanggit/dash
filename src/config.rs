use crate::{caching, spotify::caching::use_server_fn};
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    env,
    path::{Path, PathBuf},
};
use time::{Duration, UtcDateTime};

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SpotifyConfig {
    #[serde(default = "default_spotify_secrets_file")]
    pub secrets_file: PathBuf,
    #[serde(default = "default_spotify_token_cache_directory")]
    pub token_cache_directory: PathBuf,
}

fn default_spotify_secrets_file() -> PathBuf {
    "~/.config/dash/spotify-secrets.json".into()
}

fn default_spotify_token_cache_directory() -> PathBuf {
    "~/.cache/dash/rspotify".into()
}

pub fn expand_tilde<'a>(path: &'a Path) -> Cow<'a, Path> {
    if path.starts_with("~") {
        let home = env::home_dir().expect("failed to get home directory");
        Cow::Owned(home.join(path.strip_prefix("~").unwrap()))
    } else {
        Cow::Borrowed(path)
    }
}

impl Default for SpotifyConfig {
    fn default() -> Self {
        Self {
            secrets_file: default_spotify_secrets_file(),
            token_cache_directory: default_spotify_token_cache_directory(),
        }
    }
}

caching!(
    config,
    Config,
    async |(), _| -> anyhow::Result<_> {
        trace!("Getting config");

        Ok(serde_json::from_str(
            &tokio::fs::read_to_string("config.json").await?,
        )?)
    },
    CONFIG,
    Duration::minutes(1)
);

structstruck::strike!(
    #[structstruck::each[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]]
    #[structstruck::each[serde(rename_all = "camelCase")]]
    pub struct Config {
      pub password_file: PathBuf,
      #[serde(default)]
      pub spotify: SpotifyConfig,
      pub mimir: struct {
        pub url: String,
      },
      pub dashboards: Vec<struct Dashboard {
        pub name: String,
        pub width: u8,
        pub elements: Vec<struct Element {
            #[serde(rename = "type")]
            pub type_: String, // TODO: add a ElementType enum and implement Deserialize for it
            pub name: String,
            pub formula: String, // TODO: maybe use a separate type here
            pub queries: Vec<struct Query {
                #[serde(rename = "type")]
                pub type_: String,
                pub name: String,
                pub content: String,
            }>,
            pub size: struct {
                pub width: u8,
                pub height: u8,
            }
        }>
      }>
    }
);

impl Config {
    fn default() -> Self {
        Self {
            password_file: "/run/secrets/dashboard-password.hash".into(),
            spotify: SpotifyConfig::default(),
            mimir: Mimir {
                url: "localhost:3001/mimir".to_string(),
            },
            dashboards: Vec::new(),
        }
    }
}
