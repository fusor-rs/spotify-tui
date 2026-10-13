//! The sections of a search besides its songs, from the query Spotify's own apps use.
use super::{Catalog, Error, SearchItem, SearchKind, SearchResults};
use serde::Deserialize;
use std::rc::Rc;

const SEARCH_URL: &str = "https://api-partner.spotify.com/pathfinder/v2/query";
const SEARCH_OPERATION: &str = "searchDesktop";
/// The persisted query of Spotify's desktop search. Spotify may retire it one day;
/// the songs still come from the search context then, and the other sections say
/// that they are unavailable.
const SEARCH_HASH: &str = "75bbf6bfcfdf85b8fc828417bfad92b7cd66bf7f556d85670f4da8292373ebec";
/// Results in each section.
pub(crate) const SECTION_LENGTH: usize = 8;

#[derive(Deserialize)]
struct Response {
    data: Data,
}

#[derive(Deserialize)]
struct Data {
    search: Sections,
}

/// Spotify answers `null` for a section it failed to search, and fills the others.
#[derive(Deserialize)]
struct Sections {
    #[serde(default)]
    artists: Option<Section<Named>>,
    #[serde(default)]
    albums: Option<Section<Album>>,
    #[serde(default)]
    playlists: Option<Section<Playlist>>,
    #[serde(default)]
    podcasts: Option<Section<Podcast>>,
}

#[derive(Deserialize)]
struct Section<T> {
    #[serde(default = "Vec::new")]
    items: Vec<T>,
}

fn items<T>(section: Option<Section<T>>) -> Vec<T> {
    section.map(|section| section.items).unwrap_or_default()
}

#[derive(Deserialize)]
struct Name {
    name: String,
}

#[derive(Deserialize)]
struct Named {
    uri: String,
    profile: Name,
}

#[derive(Deserialize)]
struct Album {
    uri: String,
    name: String,
    artists: Section<Named>,
}

#[derive(Deserialize)]
struct Playlist {
    uri: String,
    name: String,
    owner: Option<Name>,
}

#[derive(Deserialize)]
struct Podcast {
    uri: String,
    name: String,
    publisher: Option<Name>,
}

impl Catalog {
    /// Artists, albums, playlists and podcasts that match `text`.
    pub(crate) async fn search(&self, text: String) -> Result<SearchResults, Error> {
        let variables = serde_json::json!({
            "searchTerm": text,
            "offset": 0,
            "limit": SECTION_LENGTH,
            "numberOfTopResults": SECTION_LENGTH,
            "includeAudiobooks": false,
            "includePreReleases": false,
            "includeAuthors": false,
            "includeArtistHasConcertsField": false,
            "includeLocalConcertsField": false,
        });
        let body = self
            .pathfinder(SEARCH_OPERATION, SEARCH_HASH, variables, Error::Search)
            .await?;
        results(&body)
    }

    /// Runs one of the persisted GraphQL queries of Spotify's desktop app, with
    /// librespot's login5 token; `failed` names a refusal.
    pub(super) async fn pathfinder(
        &self,
        operation: &'static str,
        hash: &'static str,
        variables: serde_json::Value,
        failed: fn(String) -> Error,
    ) -> Result<Vec<u8>, Error> {
        let (session, http) = (self.session.clone(), self.http.clone());
        super::catalog::run(async move {
            let token = session.login5().auth_token().await?.access_token;
            let client_token = session.spclient().client_token().await?;
            let request = serde_json::json!({
                "operationName": operation,
                "variables": variables,
                "extensions": { "persistedQuery": { "version": 1, "sha256Hash": hash } },
            });
            let response = http
                .post(SEARCH_URL)
                .bearer_auth(token)
                .header("client-token", client_token)
                .json(&request)
                .send()
                .await?;
            let status = response.status();
            if !status.is_success() {
                return Err(failed(status.to_string()));
            }
            Ok(response.bytes().await?.to_vec())
        })
        .await
    }
}

fn results(body: &[u8]) -> Result<SearchResults, Error> {
    let sections = serde_json::from_slice::<Response>(body)?.data.search;
    let item = |kind, uri: String, name: String, detail: Option<String>| {
        Rc::new(SearchItem {
            kind,
            uri,
            name,
            detail: detail.unwrap_or_default(),
        })
    };
    Ok(SearchResults {
        artists: items(sections.artists)
            .into_iter()
            .map(|artist| item(SearchKind::Artist, artist.uri, artist.profile.name, None))
            .collect(),
        albums: items(sections.albums)
            .into_iter()
            .map(|album| {
                let artists = album
                    .artists
                    .items
                    .into_iter()
                    .map(|artist| artist.profile.name)
                    .collect::<Vec<_>>()
                    .join(", ");
                item(SearchKind::Album, album.uri, album.name, Some(artists))
            })
            .collect(),
        playlists: items(sections.playlists)
            .into_iter()
            .map(|playlist| {
                let owner = playlist.owner.map(|owner| owner.name);
                item(SearchKind::Playlist, playlist.uri, playlist.name, owner)
            })
            .collect(),
        podcasts: items(sections.podcasts)
            .into_iter()
            .map(|podcast| {
                let publisher = podcast.publisher.map(|publisher| publisher.name);
                item(SearchKind::Podcast, podcast.uri, podcast.name, publisher)
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::results;
    use crate::spotify::SearchKind;

    #[test]
    fn sections_keep_their_order_and_details() {
        let body = br#"{"data":{"search":{
            "artists":{"items":[{"uri":"spotify:artist:a","profile":{"name":"Daft Punk"}}]},
            "albums":{"totalCount":1,"items":[{"uri":"spotify:album:d","name":"Discovery",
                "artists":{"items":[{"uri":"spotify:artist:a","profile":{"name":"Daft Punk"}},
                                    {"uri":"spotify:artist:b","profile":{"name":"Romanthony"}}]}}]},
            "playlists":{"items":[{"uri":"spotify:playlist:p","name":"Radio","owner":{"name":"Spotify"}},
                                  {"uri":"spotify:playlist:q","name":"Mine"}]},
            "podcasts":{"items":[]},
            "tracks":{"items":[]}
        }}}"#;
        let found = results(body).expect("a valid answer");
        assert_eq!(found.artists[0].name, "Daft Punk");
        assert_eq!(found.artists[0].kind, SearchKind::Artist);
        assert_eq!(found.albums[0].detail, "Daft Punk, Romanthony");
        assert_eq!(found.playlists[0].detail, "Spotify");
        assert_eq!(found.playlists[1].detail, "");
        assert!(found.podcasts.is_empty());
    }

    #[test]
    fn a_failed_section_leaves_the_others() {
        let body = br#"{"data":{"search":{
            "artists":{"items":[{"uri":"spotify:artist:a","profile":{"name":"Angele"}}]},
            "podcasts":null,"topResults":null
        }},"errors":[{"message":"Exception while fetching data (/search/topResults/featured) : Server error"}]}"#;
        let found = results(body).expect("a partial answer");
        assert_eq!(found.artists.len(), 1);
        assert!(found.albums.is_empty() && found.podcasts.is_empty());
        assert!(results(b"{}").is_err());
    }
}
