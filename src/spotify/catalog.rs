use super::{Album, Artist, Error, Playlist, Track};
use http::Method;
use librespot_core::{Session, SpotifyId, SpotifyUri};
use librespot_protocol::{
    extended_metadata::{BatchedEntityRequest, EntityRequest, ExtensionQuery},
    extension_kind::ExtensionKind,
    metadata,
    playlist4_external::SelectedListContent,
};
use protobuf::{EnumOrUnknown, Message};
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    fmt::Write,
};

const ROOTLIST_LENGTH: usize = 1000;
const TRACK_PREFIX: &str = "spotify:track:";
const EPISODE_PREFIX: &str = "spotify:episode:";
const PLAYLIST_PREFIX: &str = "spotify:playlist:";
const SEARCH_PREFIX: &str = "spotify:search:";
const STATUS_OK: i32 = 200;

/// Reads the library and catalog through the same session that plays audio.
#[derive(Clone)]
pub(crate) struct Catalog {
    session: Session,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecentlyPlayed {
    play_contexts: Vec<PlayContext>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlayContext {
    last_played_track_uri: Option<String>,
}

impl Catalog {
    pub(crate) fn new(session: Session) -> Self {
        Self { session }
    }

    pub(crate) fn liked_uri(&self) -> String {
        format!("spotify:user:{}:collection", self.session.username())
    }

    pub(crate) async fn playlists(&self) -> Result<Vec<Playlist>, Error> {
        let session = self.session.clone();
        run(async move {
            let bytes = session
                .spclient()
                .get_rootlist(0, Some(ROOTLIST_LENGTH))
                .await?;
            let rootlist = SelectedListContent::parse_from_bytes(&bytes)?;
            let contents = &rootlist.contents;
            Ok(contents
                .items
                .iter()
                .zip(&contents.meta_items)
                .filter(|(item, _)| item.uri().starts_with(PLAYLIST_PREFIX))
                .filter_map(|(item, meta)| {
                    Some(Playlist {
                        uri: item.uri().to_owned(),
                        name: meta.attributes.name.clone()?,
                    })
                })
                .collect())
        })
        .await
    }

    /// The tracks of a playlist, album, artist, the liked songs or a search.
    pub(crate) async fn context_tracks(&self, uri: String) -> Result<Vec<String>, Error> {
        let session = self.session.clone();
        run(async move {
            let context = session.spclient().get_context(&uri).await?;
            let uris = context
                .pages
                .iter()
                .flat_map(|page| &page.tracks)
                .map(|track| track.uri().to_owned());
            Ok(unique_tracks(uris))
        })
        .await
    }

    /// The last track played in each recently played album, playlist or artist.
    pub(crate) async fn recent_tracks(&self) -> Result<Vec<String>, Error> {
        let session = self.session.clone();
        run(async move {
            let path = format!(
                "/recently-played/v3/user/{}/recently-played?format=json&offset=0&limit=50&filter=default",
                session.username()
            );
            let body = session
                .spclient()
                .request_as_json(&Method::GET, &path, None, None)
                .await?;
            let recent: RecentlyPlayed = serde_json::from_slice(&body)?;
            let uris = recent
                .play_contexts
                .into_iter()
                .filter_map(|context| context.last_played_track_uri);
            Ok(unique_tracks(uris))
        })
        .await
    }

    /// The tracks and episodes of a playlist. Spotify's context of a playlist leaves its
    /// episodes out, and is empty for a playlist of podcasts, so the playlist itself is read.
    pub(crate) async fn playlist_tracks(&self, uri: String) -> Result<Vec<String>, Error> {
        let session = self.session.clone();
        run(async move {
            let id = SpotifyId::from_base62(uri.strip_prefix(PLAYLIST_PREFIX).unwrap_or(&uri))?;
            let bytes = session.spclient().get_playlist(&id).await?;
            let playlist = SelectedListContent::parse_from_bytes(&bytes)?;
            let uris = playlist
                .contents
                .items
                .iter()
                .map(|item| item.uri().to_owned());
            Ok(unique_tracks(uris))
        })
        .await
    }

    /// Looks up many tracks and episodes in one request, in the order of `uris`;
    /// unavailable ones are left out.
    pub(crate) async fn tracks(&self, uris: Vec<String>) -> Result<Vec<Track>, Error> {
        let session = self.session.clone();
        run(async move {
            let request = BatchedEntityRequest {
                entity_request: uris.iter().map(|uri| track_request(uri)).collect(),
                ..BatchedEntityRequest::default()
            };
            let response = session.spclient().get_extended_metadata(request).await?;
            let mut found = response
                .extended_metadata
                .iter()
                .flat_map(|array| &array.extension_data)
                .filter(|data| data.header.status_code == STATUS_OK)
                .map(|data| {
                    let uri = &data.entity_uri;
                    Ok((uri.clone(), describe(uri, &data.extension_data.value)?))
                })
                .collect::<Result<HashMap<_, _>, Error>>()?;
            Ok(uris.iter().filter_map(|uri| found.remove(uri)).collect())
        })
        .await
    }
}

/// The context URI that searches the catalog for `text`.
pub(crate) fn search_uri(text: &str) -> String {
    let mut uri = String::from(SEARCH_PREFIX);
    for byte in text.trim().bytes() {
        match byte {
            b' ' => uri.push('+'),
            _ if byte.is_ascii_alphanumeric() => uri.push(char::from(byte)),
            _ => {
                write!(uri, "%{byte:02X}").expect("writing to a String cannot fail");
            }
        }
    }
    uri
}

/// librespot's requests are `Send`; they run on the Tokio runtime, not the UI thread.
async fn run<T: Send + 'static>(
    task: impl Future<Output = Result<T, Error>> + Send + 'static,
) -> Result<T, Error> {
    tokio::spawn(task).await?
}

/// The tracks and episodes among `uris`, each once, in order.
fn unique_tracks(uris: impl Iterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    uris.filter(|uri| {
        (uri.starts_with(TRACK_PREFIX) || uri.starts_with(EPISODE_PREFIX))
            && seen.insert(uri.clone())
    })
    .collect()
}

fn track_request(uri: &str) -> EntityRequest {
    let kind = if uri.starts_with(EPISODE_PREFIX) {
        ExtensionKind::EPISODE_V4
    } else {
        ExtensionKind::TRACK_V4
    };
    EntityRequest {
        entity_uri: uri.to_owned(),
        query: vec![ExtensionQuery {
            extension_kind: EnumOrUnknown::new(kind),
            ..ExtensionQuery::default()
        }],
        ..EntityRequest::default()
    }
}

fn track(uri: &str, message: &metadata::Track) -> Result<Track, Error> {
    let artists = message
        .artist
        .iter()
        .map(|artist| {
            Ok(Artist {
                uri: SpotifyUri::try_from(artist)?.to_uri()?,
                name: artist.name().to_owned(),
            })
        })
        .collect::<Result<_, Error>>()?;
    Ok(Track {
        uri: uri.to_owned(),
        name: message.name().to_owned(),
        artists,
        album: Album {
            uri: SpotifyUri::try_from(&*message.album)?.to_uri()?,
            name: message.album.name().to_owned(),
        },
        duration_ms: u64::from(message.duration().unsigned_abs()),
    })
}

/// The track or episode that `bytes` describe.
fn describe(uri: &str, bytes: &[u8]) -> Result<Track, Error> {
    if uri.starts_with(EPISODE_PREFIX) {
        episode(uri, &metadata::Episode::parse_from_bytes(bytes)?)
    } else {
        track(uri, &metadata::Track::parse_from_bytes(bytes)?)
    }
}

/// An episode lists like a track whose album is its show, and has no artists.
fn episode(uri: &str, message: &metadata::Episode) -> Result<Track, Error> {
    Ok(Track {
        uri: uri.to_owned(),
        name: message.name().to_owned(),
        artists: Vec::new(),
        album: Album {
            uri: SpotifyUri::try_from(&*message.show)?.to_uri()?,
            name: message.show.name().to_owned(),
        },
        duration_ms: u64::from(message.duration().unsigned_abs()),
    })
}

#[cfg(test)]
mod tests {
    use super::{search_uri, unique_tracks};

    #[test]
    fn search_uris_encode_the_query() {
        assert_eq!(search_uri(" daft punk "), "spotify:search:daft+punk");
        assert_eq!(search_uri("AC/DC & co"), "spotify:search:AC%2FDC+%26+co");
        assert_eq!(search_uri("beyoncé"), "spotify:search:beyonc%C3%A9");
    }

    #[test]
    fn contexts_keep_each_track_once_in_order() {
        let uris = [
            "spotify:track:b",
            "spotify:episode:x",
            "spotify:local:artist:album:name:60",
            "spotify:track:a",
            "spotify:track:b",
            "spotify:episode:x",
        ]
        .map(String::from);
        assert_eq!(
            unique_tracks(uris.into_iter()),
            ["spotify:track:b", "spotify:episode:x", "spotify:track:a"]
        );
    }
}
