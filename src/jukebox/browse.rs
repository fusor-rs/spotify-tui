use super::{
    Jukebox,
    listing::{Listing, Loading, Row, Source},
};
use crate::spotify::{self, Catalog, Play, search_uri};
use hypercmd::{Error, Event, EventPayload, Input, Key};
use std::rc::Rc;

/// Tracks whose details load in one request.
const CHUNK: usize = 100;
/// Rows below the window that load before they scroll into view.
const PREFETCH_ROWS: usize = 30;
const HEADER_ROWS: usize = 1;

impl Jukebox {
    /// Opens a library section, forgetting where the user drilled down from.
    pub(super) fn open(self: &Rc<Self>, source: Source) {
        self.history.borrow_mut().clear();
        self.show(source);
    }

    pub(super) fn open_shelf(self: &Rc<Self>, source: Source) {
        self.open(source);
        self.focus("tracks");
    }

    fn drill(self: &Rc<Self>, source: Source) {
        let current = (self.listing.get_untracked(), self.window.get_untracked());
        self.history.borrow_mut().push(current);
        self.show(source);
    }

    pub(super) fn back(self: &Rc<Self>) -> bool {
        let Some((listing, window)) = self.history.borrow_mut().pop() else {
            return false;
        };
        self.generation.set(self.generation.get() + 1);
        let interrupted = listing.loading == Loading::Busy;
        self.listing.set(listing);
        self.window.set(window);
        if interrupted {
            self.load_more();
        }
        true
    }

    fn show(self: &Rc<Self>, source: Source) {
        let Some(catalog) = self.catalog() else {
            return;
        };
        let generation = self.next_generation();
        self.listing.set(Listing::new(source.clone()));
        self.window.update(|window| window.select(0, 0));
        let task = async move { resolve(&catalog, &source).await };
        self.load(generation, task, |jukebox, uris| {
            jukebox.listing.update(|listing| listing.uris = uris.into());
            jukebox.load_more();
        });
    }

    /// Loads the details of the next tracks, if the window is close to the loaded end.
    fn load_more(self: &Rc<Self>) {
        let Some(catalog) = self.catalog() else {
            return;
        };
        let end = self.window.with_untracked(|window| window.end()) + PREFETCH_ROWS;
        let chunk = self.listing.update(|listing| {
            if listing.requested >= end.min(listing.uris.len()) {
                listing.loading = Loading::Idle;
                return None;
            }
            let start = listing.requested;
            listing.requested = (start + CHUNK).min(listing.uris.len());
            listing.loading = Loading::Busy;
            Some(listing.uris[start..listing.requested].to_vec())
        });
        let Some(chunk) = chunk else {
            return;
        };
        let generation = self.next_generation();
        let task = async move { catalog.tracks(chunk).await };
        self.load(generation, task, |jukebox, tracks| {
            jukebox
                .listing
                .update(|listing| listing.tracks.extend(tracks.into_iter().map(Rc::new)));
            jukebox.load_more();
        });
    }

    fn next_generation(&self) -> u64 {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        generation
    }

    /// Runs a listing load, dropping its result if the user moved on to another list.
    fn load<T: 'static>(
        self: &Rc<Self>,
        generation: u64,
        task: impl Future<Output = Result<T, spotify::Error>> + 'static,
        apply: impl FnOnce(&Rc<Self>, T) + 'static,
    ) {
        let jukebox = self.clone();
        let started = self.services.spawn(&self.owner, async move {
            let result = task.await;
            if jukebox.generation.get() != generation {
                return;
            }
            match result {
                Ok(value) => apply(&jukebox, value),
                Err(error) => jukebox
                    .listing
                    .update(|listing| listing.loading = Loading::Failed(error.to_string())),
            }
        });
        if let Err(error) = started {
            self.warn(error.to_string());
        }
    }

    pub(super) fn load_playlists(self: &Rc<Self>) {
        let Some(catalog) = self.catalog() else {
            return;
        };
        let playlists = self.playlists.clone();
        let started = self.spawn(async move {
            let loaded = catalog.playlists().await?;
            playlists.set(loaded.into_iter().map(Rc::new).collect());
            Ok(())
        });
        if let Err(error) = started {
            self.warn(error.to_string());
        }
    }

    pub(super) fn submit_search(self: &Rc<Self>, event: &Event) {
        let EventPayload::Input(Input::Key { key, .. }) = event.payload else {
            return;
        };
        match key {
            Key::Enter => {
                let text = self.search.with_untracked(|text| text.trim().to_owned());
                if !text.is_empty() {
                    self.open(Source::Search(text));
                    self.focus("tracks");
                }
            }
            Key::Escape | Key::Down => self.focus("tracks"),
            _ => return,
        }
        event.prevent_default();
    }

    pub(super) fn start_search(self: &Rc<Self>) {
        if !self.searching() {
            self.open(Source::Search(String::new()));
        }
        self.focus("search");
    }

    pub(super) fn searching(&self) -> bool {
        self.listing
            .with(|listing| matches!(listing.source, Source::Search(_)))
    }

    pub(super) fn navigate(self: &Rc<Self>, event: &Event) -> Result<(), Error> {
        match event.payload {
            EventPayload::Input(Input::Key {
                key: Key::Enter, ..
            }) => self.play_selected()?,
            EventPayload::Input(Input::Key { key, modifiers, .. })
                if !modifiers.control && !modifiers.alt && !modifiers.super_key =>
            {
                if !matches!(
                    key,
                    Key::Up | Key::Down | Key::PageUp | Key::PageDown | Key::Home | Key::End
                ) {
                    return Ok(());
                }
                self.move_selection(key);
            }
            EventPayload::Input(Input::Scroll { rows, .. }) => {
                let key = if rows < 0 { Key::Up } else { Key::Down };
                for _ in 0..rows.unsigned_abs() {
                    self.move_selection(key);
                }
            }
            _ => return Ok(()),
        }
        event.prevent_default();
        Ok(())
    }

    fn move_selection(self: &Rc<Self>, key: Key) {
        let length = self.listing.with_untracked(|listing| listing.tracks.len());
        self.window.update(|window| window.navigate(key, length));
        if self
            .listing
            .with_untracked(|listing| listing.loading == Loading::Idle)
        {
            self.load_more();
        }
    }

    pub(super) fn resize(self: &Rc<Self>, event: &Event) {
        if let EventPayload::Resize { height, .. } = event.payload {
            let rows = usize::from(height).saturating_sub(HEADER_ROWS);
            self.window.update(|window| window.resize(rows));
            if self
                .listing
                .with_untracked(|listing| listing.loading == Loading::Idle)
            {
                self.load_more();
            }
        }
    }

    fn selected(&self) -> Option<Rc<crate::spotify::Track>> {
        let index = self.window.with_untracked(|window| window.selected);
        self.listing
            .with_untracked(|listing| listing.tracks.get(index).cloned())
    }

    pub(super) fn play_selected(&self) -> Result<(), Error> {
        let Some(track) = self.selected() else {
            return Ok(());
        };
        let Some(catalog) = self.catalog() else {
            return Ok(());
        };
        let play = self.listing.with_untracked(|listing| {
            let track = track.uri.clone();
            match &listing.source {
                Source::Liked => Play::Context {
                    uri: catalog.liked_uri(),
                    track,
                },
                Source::Playlist(playlist) => Play::Context {
                    uri: playlist.uri.clone(),
                    track,
                },
                Source::Album(album) => Play::Context {
                    uri: album.uri.clone(),
                    track,
                },
                Source::Artist(artist) => Play::Context {
                    uri: artist.uri.clone(),
                    track,
                },
                Source::Search(_) | Source::Recent => Play::Tracks {
                    uris: listing.uris.to_vec(),
                    track,
                },
            }
        });
        let mode = self.mode();
        self.with_speaker(|speaker| speaker.play(play, mode));
        Ok(())
    }

    pub(super) fn open_album(self: &Rc<Self>) {
        match self.selected() {
            Some(track) => self.drill(Source::Album(Rc::new(track.album.clone()))),
            None => self.inform("Select a track to open its album."),
        }
    }

    pub(super) fn open_artist(self: &Rc<Self>) {
        match self
            .selected()
            .and_then(|track| track.artists.first().cloned())
        {
            Some(artist) => self.drill(Source::Artist(Rc::new(artist))),
            None => self.inform("Select a track to open its artist."),
        }
    }

    pub(super) fn rows(&self) -> Vec<Row> {
        let window = self.window.get();
        let playing = self.playing_uri();
        self.listing
            .with(|listing| listing.rows(window, playing.as_deref()))
    }

    pub(super) fn listing_title(&self) -> String {
        self.listing.with(|listing| listing.source.title())
    }

    pub(super) fn listing_summary(&self) -> String {
        self.listing.with(Listing::summary)
    }

    pub(super) fn listing_empty(&self) -> Option<&'static str> {
        self.listing.with(|listing| {
            (listing.uris.is_empty() && listing.loading == Loading::Idle)
                .then(|| listing.source.empty_message())
        })
    }

    pub(super) fn is_current(&self, source: &Source) -> bool {
        self.listing.with(|listing| &listing.source == source)
    }

    pub(super) fn position(&self) -> String {
        let window = self.window.get();
        self.listing.with(|listing| {
            if listing.tracks.is_empty() {
                return String::new();
            }
            let back = if self.history.borrow().is_empty() {
                ""
            } else {
                " · Esc back"
            };
            format!("{} of {}{back}", window.selected + 1, listing.uris.len())
        })
    }
}

async fn resolve(catalog: &Catalog, source: &Source) -> Result<Vec<String>, spotify::Error> {
    let context = match source {
        Source::Search(text) if text.is_empty() => return Ok(Vec::new()),
        Source::Recent => return catalog.recent_tracks().await,
        Source::Search(text) => search_uri(text),
        Source::Liked => catalog.liked_uri(),
        Source::Playlist(playlist) => playlist.uri.clone(),
        Source::Album(album) => album.uri.clone(),
        Source::Artist(artist) => artist.uri.clone(),
    };
    catalog.context_tracks(context).await
}
