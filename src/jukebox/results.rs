//! The artists, albums, playlists and podcasts a search finds, below its songs.
use super::{Jukebox, listing::Source};
use crate::spotify::{Album, Artist, Playlist, SearchItem, SearchKind, SearchResults};
use hypercmd::{Event, EventPayload, Input, Key, Node};
use std::rc::Rc;

/// The boxes below the tracks: each title with its results.
pub(crate) type Boxes = Vec<(&'static str, Vec<Rc<SearchItem>>)>;

/// What the boxes show for the search or artist in the track list.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Found {
    Loading(String),
    Ready(String, Rc<Boxes>),
    Failed(String, String),
}

/// One box below the songs.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ResultSection {
    /// Changes with the search and its state, so that the box is built again: the
    /// rows of a box kept across an update would keep showing the old ones.
    pub(crate) key: String,
    pub(crate) title: &'static str,
    pub(crate) rows: Vec<Rc<ResultRow>>,
    /// Shown instead of rows: loading, nothing found or a failure.
    pub(crate) note: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ResultRow {
    pub(crate) label: String,
    pub(crate) item: Rc<SearchItem>,
}

impl Found {
    fn query(&self) -> &str {
        match self {
            Self::Loading(query) | Self::Ready(query, _) | Self::Failed(query, _) => query,
        }
    }
}

/// The boxes of a catalog search, in the order they show.
pub(crate) const CATALOG_BOXES: [&str; 4] = ["Artists", "Albums", "Playlists", "Podcasts"];

/// The boxes of a catalog search's results.
pub(crate) fn catalog_boxes(results: &SearchResults) -> Boxes {
    CATALOG_BOXES
        .into_iter()
        .zip([
            &results.artists,
            &results.albums,
            &results.playlists,
            &results.podcasts,
        ])
        .map(|(title, items)| (title, items.clone()))
        .collect()
}

/// The boxes for `found`, titled `titles` until it is ready.
pub(crate) fn sections(found: &Found, titles: &[&'static str]) -> Vec<ResultSection> {
    let (state, boxes): (_, Boxes) = match found {
        Found::Loading(_) => (
            "loading",
            titles.iter().map(|title| (*title, Vec::new())).collect(),
        ),
        Found::Failed(..) => (
            "failed",
            titles.iter().map(|title| (*title, Vec::new())).collect(),
        ),
        Found::Ready(_, boxes) => ("ready", boxes.as_ref().clone()),
    };
    boxes
        .into_iter()
        .map(|(title, items)| {
            let note = match found {
                Found::Loading(_) => "Searching…".to_owned(),
                Found::Failed(_, error) => error.clone(),
                Found::Ready(..) if items.is_empty() => format!("No {}", title.to_lowercase()),
                Found::Ready(..) => String::new(),
            };
            ResultSection {
                key: format!("{title}\n{state}\n{}", found.query()),
                title,
                rows: items.iter().map(|item| Rc::new(row(item))).collect(),
                note,
            }
        })
        .collect()
}

fn row(item: &Rc<SearchItem>) -> ResultRow {
    let label = if item.detail.is_empty() {
        item.name.clone()
    } else {
        format!("{} · {}", item.name, item.detail)
    };
    ResultRow {
        label,
        item: item.clone(),
    }
}

/// The list a result opens.
fn source(item: &SearchItem) -> Source {
    let (uri, name) = (item.uri.clone(), item.name.clone());
    match item.kind {
        SearchKind::Artist => Source::Artist(Rc::new(Artist { uri, name })),
        SearchKind::Album => Source::Album(Rc::new(Album { uri, name })),
        SearchKind::Playlist => Source::Playlist(Rc::new(Playlist { uri, name })),
        SearchKind::Podcast => Source::Show(Rc::new(Album { uri, name })),
    }
}

impl Jukebox {
    /// Looks for the other sections of a search, unless they are known.
    pub(super) fn load_results(self: &Rc<Self>, text: &str) {
        let known = self
            .found
            .with_untracked(|found| found.as_ref().is_some_and(|found| found.query() == text));
        let Some(catalog) = self.catalog() else {
            return;
        };
        if known {
            return;
        }
        self.found.set(Some(Found::Loading(text.to_owned())));
        let (found, text) = (self.found.clone(), text.to_owned());
        let started = self.services.spawn(&self.owner, async move {
            let outcome = match catalog.search(text.clone()).await {
                Ok(results) => Found::Ready(text.clone(), Rc::new(catalog_boxes(&results))),
                Err(error) => Found::Failed(text.clone(), error.to_string()),
            };
            // A newer search may have started meanwhile.
            if found.with_untracked(|current| current.as_ref().map(Found::query) == Some(&text)) {
                found.set(Some(outcome));
            }
        });
        if let Err(error) = started {
            self.warn(error.to_string());
        }
    }

    /// The boxes of the search or artist in the track list.
    fn current_found(&self) -> Option<Found> {
        enum Page {
            Search(String),
            Artist(String),
        }
        let page = self.listing.with(|listing| match &listing.source {
            Source::Search(text) if !text.is_empty() => Some(Page::Search(text.clone())),
            // The boxes follow the tracks, which load the artist's page first.
            Source::Artist(artist) if !listing.uris.is_empty() => {
                Some(Page::Artist(artist.uri.clone()))
            }
            _ => None,
        })?;
        match page {
            Page::Search(text) => self
                .found
                .with(|found| found.clone().filter(|found| found.query() == text)),
            Page::Artist(uri) => self.artist_found(&uri),
        }
    }

    pub(super) fn results_shown(&self) -> bool {
        self.current_found().is_some()
    }

    pub(super) fn result_sections(&self) -> Vec<Rc<ResultSection>> {
        self.current_found()
            .map(|found| {
                sections(&found, &CATALOG_BOXES)
                    .into_iter()
                    .map(Rc::new)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn open_result(self: &Rc<Self>, item: &SearchItem) {
        self.drill(source(item));
        self.focus("tracks");
    }

    /// Tab from the track list goes to the first result.
    pub(super) fn enter_results(&self) -> bool {
        let first = self.result_boxes().into_iter().flatten().next();
        first.inspect(Node::request_focus).is_some()
    }

    /// `j` and `k` move within a box, `h` and `l` to the box beside it at the
    /// same row, and Tab goes back to the track list.
    pub(super) fn results_key(&self, event: &Event) {
        let EventPayload::Input(Input::Key { key, modifiers, .. }) = event.payload else {
            return;
        };
        if modifiers.control || modifiers.alt || modifiers.super_key {
            return;
        }
        let boxes = self.result_boxes();
        let Some((column, row)) = boxes.iter().enumerate().find_map(|(column, buttons)| {
            let row = buttons.iter().position(|button| *button == event.target)?;
            Some((column, row))
        }) else {
            return;
        };
        let target = match key {
            Key::Tab if !modifiers.shift => {
                self.focus("tracks");
                event.prevent_default();
                return;
            }
            Key::Char('j') | Key::Down => boxes[column].get(row + 1),
            Key::Char('k') | Key::Up => row.checked_sub(1).and_then(|row| boxes[column].get(row)),
            Key::Char('l') | Key::Right => beside(&boxes, column + 1..boxes.len(), row),
            Key::Char('h') | Key::Left => beside(&boxes, (0..column).rev(), row),
            _ => return,
        };
        if let Some(button) = target {
            button.request_focus();
        }
        event.prevent_default();
    }

    /// The buttons of each box that has any, left to right.
    fn result_boxes(&self) -> Vec<Vec<Node>> {
        let Some(results) = self
            .root
            .borrow()
            .as_ref()
            .and_then(|root| root.find("results"))
        else {
            return Vec::new();
        };
        results
            .children()
            .iter()
            .map(|section| {
                section
                    .descendants()
                    .filter(|node| node.tag() == "button")
                    .collect::<Vec<_>>()
            })
            .filter(|buttons| !buttons.is_empty())
            .collect()
    }
}

/// The button at `row`, or the last one, of the first box among `columns`.
fn beside(
    boxes: &[Vec<Node>],
    mut columns: impl Iterator<Item = usize>,
    row: usize,
) -> Option<&Node> {
    let buttons = &boxes[columns.next()?];
    buttons.get(row).or(buttons.last())
}

#[cfg(test)]
mod tests {
    use super::{CATALOG_BOXES, Found, catalog_boxes, sections, source};
    use crate::{
        jukebox::listing::Source,
        spotify::{SearchItem, SearchKind, SearchResults},
    };
    use std::rc::Rc;

    fn item(kind: SearchKind, name: &str, detail: &str) -> Rc<SearchItem> {
        Rc::new(SearchItem {
            kind,
            uri: format!("spotify:x:{name}"),
            name: name.into(),
            detail: detail.into(),
        })
    }

    #[test]
    fn sections_name_what_they_lack() {
        let results = SearchResults {
            albums: vec![item(SearchKind::Album, "Discovery", "Daft Punk")],
            artists: vec![item(SearchKind::Artist, "Daft Punk", "")],
            ..SearchResults::default()
        };
        let shown = sections(
            &Found::Ready("daft".into(), Rc::new(catalog_boxes(&results))),
            &CATALOG_BOXES,
        );
        let titles: Vec<_> = shown.iter().map(|section| section.title).collect();
        assert_eq!(titles, ["Artists", "Albums", "Playlists", "Podcasts"]);
        assert_eq!(shown[0].rows[0].label, "Daft Punk");
        assert_eq!(shown[1].rows[0].label, "Discovery · Daft Punk");
        assert_eq!(shown[2].note, "No playlists");
        let loading = sections(&Found::Loading("daft".into()), &CATALOG_BOXES);
        assert!(loading.iter().all(|section| section.note == "Searching…"));
        assert_ne!(loading[0].key, shown[0].key);
    }

    #[test]
    fn podcasts_open_as_shows() {
        let podcast = item(SearchKind::Podcast, "Show", "Host");
        assert!(matches!(source(&podcast), Source::Show(show) if show.name == "Show"));
        let artist = item(SearchKind::Artist, "Band", "");
        assert!(matches!(source(&artist), Source::Artist(_)));
    }
}
