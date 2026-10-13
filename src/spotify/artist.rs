//! An artist's page: profile, numbers, popular tracks and discography, from the
//! query Spotify's desktop app uses for it.
use super::{ArtistPage, Catalog, Error, SearchItem, SearchKind, catalog::plain_text};
use serde::Deserialize;
use std::rc::Rc;

const OPERATION: &str = "queryArtistOverview";
/// The persisted query of Spotify's artist page. If Spotify retires it, the artist's
/// tracks still come from the artist's context, without the page around them.
const HASH: &str = "79a4a9d7c3a3781d801e62b62ef11c7ee56fce2626772eb26cd20c69f84b3f49";
/// The size of avatar that is sharp enough for the header.
const SHARP_ENOUGH: u32 = 300;

#[derive(Deserialize)]
struct Response {
    data: Data,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Data {
    artist_union: Artist,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Artist {
    uri: String,
    profile: Profile,
    visuals: Visuals,
    stats: Stats,
    discography: Discography,
    related_content: Related,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Profile {
    name: String,
    verified: bool,
    biography: Option<Text>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Text {
    text: String,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Visuals {
    avatar_image: Option<Images>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Images {
    sources: Vec<Image>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Image {
    url: String,
    width: Option<u32>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Stats {
    followers: Option<u64>,
    monthly_listeners: Option<u64>,
    world_rank: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Discography {
    top_tracks: Items<TopTrack>,
    albums: Items<Releases>,
    singles: Items<Releases>,
    compilations: Items<Releases>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Related {
    appears_on: Items<Releases>,
    related_artists: Items<RelatedArtist>,
}

#[derive(Deserialize)]
#[serde(default)]
struct Items<T> {
    items: Vec<T>,
}

impl<T> Default for Items<T> {
    fn default() -> Self {
        Self { items: Vec::new() }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct TopTrack {
    track: Option<Track>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Track {
    uri: String,
    /// A number, sent as a string.
    playcount: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Releases {
    releases: Items<Release>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Release {
    uri: String,
    name: String,
    date: Option<Date>,
    artists: Option<Items<Named>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Date {
    year: Option<u32>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Named {
    profile: Name,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Name {
    name: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RelatedArtist {
    uri: String,
    profile: Name,
}

impl Catalog {
    /// The page of the artist `uri`.
    pub(crate) async fn artist_page(&self, uri: String) -> Result<ArtistPage, Error> {
        let variables = serde_json::json!({ "uri": uri, "locale": "", "includePrerelease": true });
        let body = self
            .pathfinder(OPERATION, HASH, variables, Error::Artist)
            .await?;
        page(&body)
    }
}

fn page(body: &[u8]) -> Result<ArtistPage, Error> {
    let artist = serde_json::from_slice::<Response>(body)?.data.artist_union;
    let mut sizes = artist
        .visuals
        .avatar_image
        .map(|images| images.sources)
        .unwrap_or_default();
    sizes.sort_by_key(|image| image.width.unwrap_or(0));
    let avatar = sizes
        .iter()
        .find(|image| image.width.unwrap_or(0) >= SHARP_ENOUGH)
        .or(sizes.last())
        .map(|image| image.url.clone());
    let discography = artist.discography;
    let mut albums = releases(discography.albums, false);
    albums.extend(releases(discography.compilations, false));
    Ok(ArtistPage {
        uri: artist.uri,
        name: artist.profile.name,
        verified: artist.profile.verified,
        monthly_listeners: artist.stats.monthly_listeners,
        followers: artist.stats.followers,
        world_rank: artist.stats.world_rank.filter(|rank| *rank > 0),
        biography: artist
            .profile
            .biography
            .map(|biography| plain_text(&biography.text))
            .unwrap_or_default(),
        avatar,
        top_tracks: discography
            .top_tracks
            .items
            .into_iter()
            .filter_map(|item| item.track)
            .map(|track| {
                let plays = track.playcount.and_then(|count| count.parse().ok());
                (track.uri, plays)
            })
            .collect(),
        albums,
        singles: releases(discography.singles, false),
        appears_on: releases(artist.related_content.appears_on, true),
        related: artist
            .related_content
            .related_artists
            .items
            .into_iter()
            .map(|related| {
                Rc::new(SearchItem {
                    kind: SearchKind::Artist,
                    uri: related.uri,
                    name: related.profile.name,
                    detail: String::new(),
                })
            })
            .collect(),
    })
}

/// The first release of each group, named with its year, or its artists for the
/// albums an artist appears on.
fn releases(groups: Items<Releases>, by_artists: bool) -> Vec<Rc<SearchItem>> {
    groups
        .items
        .into_iter()
        .filter_map(|group| group.releases.items.into_iter().next())
        .map(|release| {
            let detail = if by_artists {
                release
                    .artists
                    .map(|artists| {
                        artists
                            .items
                            .into_iter()
                            .map(|artist| artist.profile.name)
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default()
            } else {
                release
                    .date
                    .and_then(|date| date.year)
                    .map(|year| year.to_string())
                    .unwrap_or_default()
            };
            Rc::new(SearchItem {
                kind: SearchKind::Album,
                uri: release.uri,
                name: release.name,
                detail,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::page;

    #[test]
    fn a_page_keeps_what_the_view_shows() {
        let body = br#"{"data":{"artistUnion":{
            "uri":"spotify:artist:a",
            "profile":{"name":"Daft Punk","verified":true,
                "biography":{"type":"BIOGRAPHY","text":"French <a href=\"x\">duo</a> &amp; more"}},
            "visuals":{"avatarImage":{"sources":[
                {"url":"big","width":640},{"url":"small","width":160},{"url":"mid","width":320}]}},
            "stats":{"followers":11844086,"monthlyListeners":28201141,"worldRank":261},
            "discography":{
                "topTracks":{"items":[{"track":{"uri":"spotify:track:t","playcount":"985907520"}},
                                      {"track":{"uri":"spotify:track:u","playcount":null}}]},
                "albums":{"items":[{"releases":{"items":[
                    {"uri":"spotify:album:d","name":"Discovery","date":{"year":2001}}]}}]},
                "singles":{"items":[]},
                "compilations":{"items":[{"releases":{"items":[
                    {"uri":"spotify:album:c","name":"Musique","date":{"year":2006}}]}}]}},
            "relatedContent":{
                "appearsOn":{"items":[{"releases":{"items":[{"uri":"spotify:album:s","name":"Starboy",
                    "artists":{"items":[{"profile":{"name":"The Weeknd"}}]}}]}}]},
                "relatedArtists":{"items":[{"uri":"spotify:artist:j","profile":{"name":"Justice"}}]}}
        }}}"#;
        let artist = page(body).expect("a valid page");
        assert_eq!(artist.name, "Daft Punk");
        assert!(artist.verified);
        assert_eq!(artist.biography, "French duo & more");
        assert_eq!(artist.avatar.as_deref(), Some("mid"));
        assert_eq!(artist.monthly_listeners, Some(28_201_141));
        assert_eq!(
            artist.top_tracks,
            [
                ("spotify:track:t".to_owned(), Some(985_907_520)),
                ("spotify:track:u".to_owned(), None)
            ]
        );
        let names: Vec<_> = artist
            .albums
            .iter()
            .map(|album| album.detail.as_str())
            .collect();
        assert_eq!(names, ["2001", "2006"]);
        assert_eq!(artist.appears_on[0].detail, "The Weeknd");
        assert_eq!(artist.related[0].name, "Justice");
        assert!(artist.singles.is_empty());
    }

    #[test]
    fn a_sparse_page_still_reads() {
        let artist = page(
            br#"{"data":{"artistUnion":{"uri":"spotify:artist:a","profile":{"name":"New"}}}}"#,
        )
        .expect("a sparse page");
        assert!(artist.top_tracks.is_empty() && artist.avatar.is_none());
    }
}
