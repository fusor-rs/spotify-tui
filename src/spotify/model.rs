use std::rc::Rc;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Artist {
    pub(crate) uri: String,
    pub(crate) name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Album {
    pub(crate) uri: String,
    pub(crate) name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Track {
    pub(crate) uri: String,
    pub(crate) name: String,
    pub(crate) artists: Vec<Artist>,
    pub(crate) album: Album,
    pub(crate) duration_ms: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Playlist {
    pub(crate) uri: String,
    pub(crate) name: String,
}

/// What a search result opens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SearchKind {
    Artist,
    Album,
    Playlist,
    Podcast,
}

/// An artist, album, playlist or podcast found by a search.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SearchItem {
    pub(crate) kind: SearchKind,
    pub(crate) uri: String,
    pub(crate) name: String,
    /// Who made it: the album's artists, the playlist's owner, the podcast's publisher.
    pub(crate) detail: String,
}

/// The sections of a search besides its songs.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SearchResults {
    pub(crate) artists: Vec<Rc<SearchItem>>,
    pub(crate) albums: Vec<Rc<SearchItem>>,
    pub(crate) playlists: Vec<Rc<SearchItem>>,
    pub(crate) podcasts: Vec<Rc<SearchItem>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum Repeat {
    #[default]
    Off,
    Context,
    Track,
}

impl Track {
    pub(crate) fn artist_names(&self) -> String {
        self.artists
            .iter()
            .map(|artist| artist.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl Repeat {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Off => Self::Context,
            Self::Context => Self::Track,
            Self::Track => Self::Off,
        }
    }
}
