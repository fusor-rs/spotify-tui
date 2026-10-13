//! An artist's page: its popular tracks, with how often they were played, and its
//! albums, singles, appearances and related artists in boxes below.
use super::{Jukebox, header::grouped, results::Found};
use crate::spotify::{self, ArtistPage, Catalog};
use std::{cell::RefCell, collections::HashMap, rc::Rc};

/// Artist pages already loaded, by artist URI.
pub(crate) type ArtistCache = Rc<RefCell<HashMap<String, Rc<ArtistPage>>>>;

/// The page of `uri`, from the cache or loaded into it.
pub(super) async fn artist_page(
    catalog: &Catalog,
    cache: &ArtistCache,
    uri: String,
) -> Result<Rc<ArtistPage>, spotify::Error> {
    if let Some(page) = cache.borrow().get(&uri).cloned() {
        return Ok(page);
    }
    let page = Rc::new(catalog.artist_page(uri.clone()).await?);
    cache.borrow_mut().insert(uri, page.clone());
    Ok(page)
}

/// The popular tracks of an artist, or all of its tracks without its page.
pub(super) async fn artist_tracks(
    catalog: &Catalog,
    cache: &ArtistCache,
    uri: String,
) -> Result<Vec<String>, spotify::Error> {
    match artist_page(catalog, cache, uri.clone()).await {
        Ok(page) if !page.top_tracks.is_empty() => {
            Ok(page.top_tracks.iter().map(|(uri, _)| uri.clone()).collect())
        }
        _ => catalog.context_tracks(uri).await,
    }
}

impl Jukebox {
    /// The boxes below an artist's popular tracks, once its page is loaded.
    pub(super) fn artist_found(&self, uri: &str) -> Option<Found> {
        let page = self.artists.borrow().get(uri).cloned()?;
        let boxes = vec![
            ("Albums", page.albums.clone()),
            ("Singles", page.singles.clone()),
            ("Appears on", page.appears_on.clone()),
            ("Fans also like", page.related.clone()),
        ];
        Some(Found::Ready(uri.to_owned(), Rc::new(boxes)))
    }

    /// How often each popular track of the artist `uri` was played, when known.
    pub(super) fn plays(&self, uri: &str) -> Option<HashMap<String, String>> {
        let page = self.artists.borrow().get(uri).cloned()?;
        Some(
            page.top_tracks
                .iter()
                .map(|(track, plays)| (track.clone(), plays.map(grouped).unwrap_or_default()))
                .collect(),
        )
    }
}
