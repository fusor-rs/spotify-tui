use crate::spotify::{Album, Artist, Playlist, Track};
use hypercmd::Key;
use std::{ops::Range, rc::Rc};

/// Something the main pane can list.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Source {
    Search(String),
    Liked,
    Recent,
    Playlist(Rc<Playlist>),
    Album(Rc<Album>),
    Artist(Rc<Artist>),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Loading {
    Idle,
    Busy,
    Failed(String),
}

/// The tracks of a source. Every track URI is known up front; details load as rows come into view.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Listing {
    pub(crate) source: Source,
    pub(crate) uris: Rc<[String]>,
    pub(crate) tracks: Vec<Rc<Track>>,
    pub(crate) requested: usize,
    pub(crate) loading: Loading,
}

/// One rendered line of the main pane.
#[derive(Clone, PartialEq)]
pub(crate) struct Row {
    pub(crate) index: usize,
    pub(crate) marker: String,
    pub(crate) title: String,
    pub(crate) artist: String,
    pub(crate) album: String,
    pub(crate) length: String,
    pub(crate) selected: bool,
    pub(crate) playing: bool,
}

/// The visible slice of a list and its selected entry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Window {
    pub(crate) selected: usize,
    top: usize,
    rows: usize,
}

impl Default for Window {
    fn default() -> Self {
        Self {
            selected: 0,
            top: 0,
            rows: 1,
        }
    }
}

impl Window {
    pub(crate) fn resize(&mut self, rows: usize) {
        self.rows = rows.max(1);
        self.reveal();
    }

    pub(crate) fn select(&mut self, index: usize, length: usize) {
        self.selected = index.min(length.saturating_sub(1));
        self.reveal();
    }

    pub(crate) fn navigate(&mut self, key: Key, length: usize) {
        let target = match key {
            Key::Up => self.selected.saturating_sub(1),
            Key::Down => self.selected + 1,
            Key::PageUp => self.selected.saturating_sub(self.rows),
            Key::PageDown => self.selected + self.rows,
            Key::Home => 0,
            Key::End => length,
            _ => return,
        };
        self.select(target, length);
    }

    pub(crate) fn visible(&self, length: usize) -> Range<usize> {
        self.top.min(length)..(self.top + self.rows).min(length)
    }

    /// The first row past the window, which should be loaded before it scrolls into view.
    pub(crate) fn end(&self) -> usize {
        self.top + self.rows
    }

    fn reveal(&mut self) {
        if self.selected < self.top {
            self.top = self.selected;
        } else if self.selected >= self.top + self.rows {
            self.top = self.selected + 1 - self.rows;
        }
    }
}

impl Source {
    pub(crate) fn title(&self) -> String {
        match self {
            Self::Search(text) if text.is_empty() => "Search".into(),
            Self::Search(text) => format!("Search · “{text}”"),
            Self::Liked => "Liked Songs".into(),
            Self::Recent => "Recently played".into(),
            Self::Playlist(playlist) => playlist.name.clone(),
            Self::Album(album) => format!("Album · {}", album.name),
            Self::Artist(artist) => format!("Artist · {}", artist.name),
        }
    }

    pub(crate) fn empty_message(&self) -> &'static str {
        match self {
            Self::Search(text) if text.is_empty() => "Type what you want to hear and press Enter.",
            Self::Search(_) => "Nothing matched. Try other words.",
            Self::Liked => "Songs you like in Spotify appear here.",
            Self::Recent => "Nothing played recently.",
            Self::Playlist(_) | Self::Album(_) | Self::Artist(_) => "No playable tracks here.",
        }
    }
}

impl Listing {
    pub(crate) fn new(source: Source) -> Self {
        Self {
            source,
            uris: Rc::from([]),
            tracks: Vec::new(),
            requested: 0,
            loading: Loading::Busy,
        }
    }

    pub(crate) fn summary(&self) -> String {
        match &self.loading {
            Loading::Busy if self.tracks.is_empty() => "Loading…".into(),
            Loading::Failed(error) => error.clone(),
            _ if self.uris.len() == 1 => "1 song".into(),
            _ => format!("{} songs", self.uris.len()),
        }
    }

    pub(crate) fn rows(&self, window: Window, playing: Option<&str>) -> Vec<Row> {
        let visible = window.visible(self.tracks.len());
        self.tracks[visible.clone()]
            .iter()
            .zip(visible)
            .map(|(track, index)| {
                let playing = playing == Some(track.uri.as_str());
                Row {
                    index,
                    marker: if playing {
                        "♫".into()
                    } else {
                        (index + 1).to_string()
                    },
                    title: track.name.clone(),
                    artist: track.artist_names(),
                    album: track.album.name.clone(),
                    length: duration(track.duration_ms),
                    selected: window.selected == index,
                    playing,
                }
            })
            .collect()
    }
}

/// Formats milliseconds as `m:ss`, or `h:mm:ss` for an hour or more.
pub(crate) fn duration(milliseconds: u64) -> String {
    let seconds = milliseconds / 1000;
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::{Key, Window, duration};

    #[test]
    fn durations_use_minutes_and_hours() {
        assert_eq!(duration(0), "0:00");
        assert_eq!(duration(65_400), "1:05");
        assert_eq!(duration(3_723_000), "1:02:03");
    }

    #[test]
    fn window_keeps_the_selection_visible() {
        let mut window = Window::default();
        window.resize(3);
        for _ in 0..4 {
            window.navigate(Key::Down, 10);
        }
        assert_eq!(window.selected, 4);
        assert_eq!(window.visible(10), 2..5);
        window.navigate(Key::End, 10);
        assert_eq!(window.visible(10), 7..10);
        window.navigate(Key::PageUp, 10);
        assert_eq!((window.selected, window.visible(10)), (6, 6..9));
        window.navigate(Key::Home, 10);
        assert_eq!(window.visible(10), 0..3);
    }

    #[test]
    fn window_clamps_to_short_lists() {
        let mut window = Window::default();
        window.resize(5);
        window.navigate(Key::PageDown, 2);
        assert_eq!((window.selected, window.visible(2)), (1, 0..2));
        window.select(9, 0);
        assert_eq!((window.selected, window.visible(0)), (0, 0..0));
    }
}
