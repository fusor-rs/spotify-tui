mod auth;
mod catalog;
mod model;
mod speaker;

pub(crate) use auth::{Authorization, Token, forget, stored_refresh_token};
pub(crate) use catalog::{Catalog, search_uri};
pub(crate) use model::{Album, Artist, Playlist, Repeat, Track};
pub(crate) use speaker::{Command, DEVICE_NAME, Mode, Play, Speaker};

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("Cannot reach Spotify; check your network connection ({0})")]
    Network(#[from] reqwest::Error),
    #[error("Spotify: {0}")]
    Librespot(#[from] librespot_core::Error),
    #[error("Cannot read Spotify's response ({0})")]
    Protobuf(#[from] protobuf::Error),
    #[error("Cannot read Spotify's response ({0})")]
    Json(#[from] serde_json::Error),
    #[error("Your Spotify login expired or was revoked; log in again")]
    Unauthorized,
    #[error("Login failed: {0}")]
    Callback(String),
    #[error("Login failed while talking to the browser: {0}")]
    Browser(#[from] std::io::Error),
    #[error("Cannot use the system keychain for your login ({0})")]
    Keychain(#[from] keyring::Error),
    #[error("No audio output is available on this computer")]
    NoAudio,
    #[error("A Spotify request stopped unexpectedly ({0})")]
    Stopped(#[from] tokio::task::JoinError),
}
