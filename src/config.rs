use crate::caching;
use crate::spotify::caching::use_server_fn;
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};
use time::Duration;
use time::UtcDateTime;
#[cfg(feature = "server")]
use tokio::sync::{Mutex, RwLock};

caching!(
    config,
    Config,
    async |(), _| {
        trace!("Getting config");

        serde_json::from_str(
            &tokio::fs::read_to_string("config.json")
                .await
                .expect("Config should be readable"),
        )
        .expect("Config should be correct")
    },
    CONFIG,
    Duration::minutes(1)
);

structstruck::strike!(
    #[structstruck::each[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]]
    #[structstruck::each[serde(rename_all = "camelCase")]]
    pub struct Config {
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
            mimir: Mimir {
                url: "localhost:3001/mimir".to_string(),
            },
            dashboards: Vec::new(),
        }
    }
}
