use std::sync::OnceLock;

use dioxus::fullstack::get_server_url;
use rspotify::{prelude::OAuthClient, scopes, AuthCodeSpotify, Config, Credentials, OAuth};

pub async fn spotify() -> AuthCodeSpotify {
    let spotify = AuthCodeSpotify::with_config(
        Credentials::from_env().expect("Failed to get credentials"),
        OAuth {
            redirect_uri: "http://127.0.0.1:8888".into(), // TODO: get the actual url
            scopes: scopes!("user-read-playback-state"),  // TODO
            ..Default::default()
        },
        Config {
            token_cached: true,
            token_refreshing: true,
            ..Default::default()
        },
    );
    let url = spotify
        .get_authorize_url(false)
        .expect("Should be able to get authorization url");

    spotify
        .prompt_for_token(&url)
        .await
        .expect("Should be able to authenticate");

    spotify
}
