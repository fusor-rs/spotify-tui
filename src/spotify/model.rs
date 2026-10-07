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
