use super::{Jukebox, listing::Source};
use crate::spotify::PlaylistDetails;
use std::rc::Rc;

impl Jukebox {
    /// Loads the header of the playlist `uri`, unless it is already there.
    pub(super) fn load_playlist_header(self: &Rc<Self>, uri: &str) {
        let known = self
            .playlist
            .with_untracked(|details| details.as_ref().is_some_and(|details| details.uri == uri));
        let Some(catalog) = self.catalog() else {
            return;
        };
        if known {
            return;
        }
        let (playlist, uri) = (self.playlist.clone(), uri.to_owned());
        let started = self.spawn(async move {
            playlist.set(Some(Rc::new(catalog.playlist_details(uri).await?)));
            Ok(())
        });
        if let Err(error) = started {
            self.warn(error.to_string());
        }
    }

    /// The details of the listed playlist, once they are known.
    fn playlist_header(&self) -> Option<Rc<PlaylistDetails>> {
        let uri = self.listing.with(|listing| match &listing.source {
            Source::Playlist(playlist) => Some(playlist.uri.clone()),
            _ => None,
        })?;
        self.playlist
            .with(|details| details.clone().filter(|details| details.uri == uri))
    }

    pub(super) fn has_playlist_header(&self) -> bool {
        self.playlist_header().is_some()
    }

    pub(super) fn playlist_name(&self) -> String {
        self.playlist_header()
            .map(|details| details.name.clone())
            .unwrap_or_default()
    }

    pub(super) fn playlist_description(&self) -> String {
        self.playlist_header()
            .map(|details| details.description.clone())
            .unwrap_or_default()
    }

    /// Who made the playlist and how many tracks it holds.
    pub(super) fn playlist_byline(&self) -> String {
        let Some(details) = self.playlist_header() else {
            return String::new();
        };
        let summary = self.listing_summary();
        [details.owner.clone(), summary]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" · ")
    }
}
