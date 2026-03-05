use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[cfg(feature = "server")]
pub async fn config() -> Config {
    serde_json::from_str(
        &tokio::fs::read_to_string("config.json")
            .await
            .expect("Config should be readable"),
    )
    .expect("Config should be correct")
}

structstruck::strike!(
    #[structstruck::each[derive(Debug, Deserialize, Clone)]]
    #[structstruck::each[serde(rename_all = "camelCase")]]
    pub struct Config {
      pub mimir: struct {
        pub url: String,
      },
      pub spotify: struct {
        pub export_json_path: PathBuf,
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
