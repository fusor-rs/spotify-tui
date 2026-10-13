//! Searching the user's own library: the Liked Songs and the playlists.
use super::{Jukebox, listing::Source, results::Found};
use crate::spotify::{self, Catalog, Playlist, SearchItem, SearchKind, SearchResults, Track};
use std::{cell::RefCell, collections::HashSet, rc::Rc};

/// Tracks whose details load in one request.
const CHUNK: usize = 100;
/// Results in each box, as in a catalog search.
const SECTION_LENGTH: usize = 8;

/// The Liked Songs with their details, loaded once for library searches.
pub(crate) type LikedCache = Rc<RefCell<Option<Rc<[Rc<Track>]>>>>;

/// The Liked Songs, from the cache or loaded into it.
pub(super) async fn liked_tracks(
    catalog: &Catalog,
    cache: &LikedCache,
) -> Result<Rc<[Rc<Track>]>, spotify::Error> {
    if let Some(tracks) = cache.borrow().clone() {
        return Ok(tracks);
    }
    let uris = catalog.context_tracks(catalog.liked_uri()).await?;
    let mut tracks = Vec::with_capacity(uris.len());
    for chunk in uris.chunks(CHUNK) {
        tracks.extend(
            catalog
                .tracks(chunk.to_vec())
                .await?
                .into_iter()
                .map(Rc::new),
        );
    }
    let tracks: Rc<[Rc<Track>]> = tracks.into();
    cache.replace(Some(tracks.clone()));
    Ok(tracks)
}

/// The URIs of the liked tracks whose title, artists or album contain `text`.
pub(super) fn matching_tracks(tracks: &[Rc<Track>], text: &str) -> Vec<String> {
    let words = words(text);
    tracks
        .iter()
        .filter(|track| {
            let haystack = fold(&format!(
                "{} {} {}",
                track.name,
                track.artist_names(),
                track.album.name
            ));
            words.iter().all(|word| haystack.contains(word.as_str()))
        })
        .map(|track| track.uri.clone())
        .collect()
}

/// The artists and albums of the matching liked tracks, and the playlists whose
/// names match, as the boxes below the tracks show them.
pub(super) fn library_results(
    tracks: &[Rc<Track>],
    playlists: &[Rc<Playlist>],
    text: &str,
) -> SearchResults {
    let words = words(text);
    let matches = |name: &str| {
        let name = fold(name);
        words.iter().all(|word| name.contains(word.as_str()))
    };
    let mut seen = HashSet::new();
    let mut item = |kind, uri: &str, name: &str, detail: String| {
        (!uri.is_empty() && seen.insert(uri.to_owned())).then(|| {
            Rc::new(SearchItem {
                kind,
                uri: uri.to_owned(),
                name: name.to_owned(),
                detail,
            })
        })
    };
    let artists = tracks
        .iter()
        .flat_map(|track| &track.artists)
        .filter(|artist| matches(&artist.name))
        .filter_map(|artist| item(SearchKind::Artist, &artist.uri, &artist.name, String::new()))
        .take(SECTION_LENGTH)
        .collect();
    let albums = tracks
        .iter()
        .filter(|track| matches(&track.album.name))
        .filter_map(|track| {
            let artists = track.artist_names();
            item(
                SearchKind::Album,
                &track.album.uri,
                &track.album.name,
                artists,
            )
        })
        .take(SECTION_LENGTH)
        .collect();
    let playlists = playlists
        .iter()
        .filter(|playlist| matches(&playlist.name))
        .filter_map(|playlist| {
            item(
                SearchKind::Playlist,
                &playlist.uri,
                &playlist.name,
                String::new(),
            )
        })
        .take(SECTION_LENGTH)
        .collect();
    SearchResults {
        artists,
        albums,
        playlists,
        podcasts: Vec::new(),
    }
}

/// The words of a search, folded like what they are looked for in.
fn words(text: &str) -> Vec<String> {
    fold(text).split_whitespace().map(str::to_owned).collect()
}

/// Lowercase without the accents of Latin letters, so that "beyonce" finds "Beyoncé".
fn fold(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|character| match character {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
            'ç' => 'c',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ñ' => 'n',
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'ý' | 'ÿ' => 'y',
            other => other,
        })
        .collect()
}

impl Jukebox {
    pub(super) fn start_library_search(self: &Rc<Self>) {
        if !self.searching_library() {
            self.open(Source::Library(String::new()));
        }
        self.focus("search");
    }

    pub(super) fn searching_library(&self) -> bool {
        self.listing
            .with(|listing| matches!(listing.source, Source::Library(_)))
    }

    /// The boxes of a library search, once the Liked Songs are loaded.
    pub(super) fn library_found(&self, text: &str) -> Option<Found> {
        let tracks = self.liked.borrow().clone()?;
        let results = self
            .playlists
            .with(|playlists| library_results(&tracks, playlists, text));
        Some(Found::Ready(text.to_owned(), Rc::new(results)))
    }
}

#[cfg(test)]
mod tests {
    use super::{fold, library_results, matching_tracks};
    use crate::spotify::{Album, Artist, Playlist, Track};
    use std::rc::Rc;

    fn track(name: &str, artist: &str, album: &str) -> Rc<Track> {
        Rc::new(Track {
            uri: format!("spotify:track:{name}"),
            name: name.into(),
            artists: vec![Artist {
                uri: format!("spotify:artist:{artist}"),
                name: artist.into(),
            }],
            album: Album {
                uri: format!("spotify:album:{album}"),
                name: album.into(),
            },
            duration_ms: 0,
        })
    }

    #[test]
    fn accents_and_case_do_not_matter() {
        assert_eq!(fold("Beyoncé À LA FOLIE"), "beyonce a la folie");
    }

    #[test]
    fn every_word_must_match_somewhere() {
        let tracks = [
            track("Flamme", "Juliette Armanet", "Brûler le feu"),
            track(
                "Le dernier jour du disco",
                "Juliette Armanet",
                "Brûler le feu",
            ),
            track("Veridis Quo", "Daft Punk", "Discovery"),
        ];
        let found = |text| matching_tracks(&tracks, text);
        assert_eq!(found("armanet bruler").len(), 2);
        assert_eq!(
            found("armanet disco"),
            ["spotify:track:Le dernier jour du disco"]
        );
        assert_eq!(found("DISCOVERY"), ["spotify:track:Veridis Quo"]);
        assert!(found("jazz").is_empty());
    }

    #[test]
    fn boxes_keep_each_artist_album_and_playlist_once() {
        let tracks = [
            track("Flamme", "Juliette Armanet", "Brûler le feu"),
            track("Qu'importe", "Juliette Armanet", "Brûler le feu"),
        ];
        let library = [Rc::new(Playlist {
            uri: "spotify:playlist:p".into(),
            name: "Juliette en boucle".into(),
        })];
        let results = library_results(&tracks, &library, "juliette");
        assert_eq!(results.artists.len(), 1);
        assert!(results.albums.is_empty());
        assert_eq!(results.playlists[0].name, "Juliette en boucle");
        assert_eq!(library_results(&tracks, &library, "bruler").albums.len(), 1);
    }
}
