//! Publishes the speaker on the session bus as an MPRIS player, so `playerctl`,
//! media keys and status bars can control it.

use super::DEVICE_NAME;
use librespot_connect::Spirc;
use librespot_metadata::audio::{AudioItem, UniqueFields};
use librespot_playback::player::{PlayerEvent, PlayerEventChannel};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use zbus::{
    Connection, connection,
    fdo::{self, RequestNameFlags},
    interface,
    object_server::{InterfaceRef, SignalEmitter},
    zvariant::{ObjectPath, OwnedValue, Value},
};

const PATH: &str = "/org/mpris/MediaPlayer2";
const BUS_NAME: &str = "org.mpris.MediaPlayer2.spt";
const NO_TRACK: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";
const TRACK_URL: &str = "https://open.spotify.com/track/";

/// Serves MPRIS for as long as the speaker reports player events.
pub(crate) fn publish(spirc: Arc<Spirc>, events: PlayerEventChannel) {
    tokio::spawn(async move {
        // Without a session bus, such as over SSH, the player works but is not published.
        if let Ok(connection) = serve(spirc).await {
            follow(&connection, events).await;
        }
    });
}

async fn serve(spirc: Arc<Spirc>) -> zbus::Result<Connection> {
    let connection = connection::Builder::session()?
        .serve_at(PATH, Root)?
        .serve_at(
            PATH,
            Player {
                spirc,
                now: Now::default(),
            },
        )?
        .build()
        .await?;
    // A second spt claims a name of its own, as MPRIS asks of multiple instances.
    let flags = RequestNameFlags::DoNotQueue.into();
    if connection
        .request_name_with_flags(BUS_NAME, flags)
        .await
        .is_err()
    {
        let instance = format!("{BUS_NAME}.instance{}", std::process::id());
        connection.request_name_with_flags(instance, flags).await?;
    }
    Ok(connection)
}

async fn follow(connection: &Connection, mut events: PlayerEventChannel) {
    let Ok(player) = connection
        .object_server()
        .interface::<_, Player>(PATH)
        .await
    else {
        return;
    };
    while let Some(event) = events.recv().await {
        let change = player.get_mut().await.now.apply(event);
        // A listener that misses one update catches up on the next.
        let _ = announce(&player, change).await;
    }
}

async fn announce(player: &InterfaceRef<Player>, change: Change) -> zbus::Result<()> {
    let emitter = player.signal_emitter();
    let player = player.get().await;
    match change {
        Change::Nothing => Ok(()),
        Change::Track => player.metadata_changed(emitter).await,
        Change::Status => player.playback_status_changed(emitter).await,
        Change::Volume => player.volume_changed(emitter).await,
        Change::Shuffle => player.shuffle_changed(emitter).await,
        Change::Loop => player.loop_status_changed(emitter).await,
        Change::Position(position) => Player::seeked(emitter, position).await,
    }
}

/// The track details MPRIS clients show.
#[derive(Clone, Debug, PartialEq)]
struct Track {
    id: String,
    uri: String,
    title: String,
    artists: Vec<String>,
    album: Option<String>,
    album_artists: Vec<String>,
    length: Duration,
    art_url: Option<String>,
}

impl Track {
    fn from_item(item: &AudioItem) -> Self {
        let (artists, album, album_artists) = match &item.unique_fields {
            UniqueFields::Track {
                artists,
                album,
                album_artists,
                ..
            } => (
                artists.iter().map(|artist| artist.name.clone()).collect(),
                Some(album.clone()),
                album_artists.clone(),
            ),
            UniqueFields::Local {
                artists,
                album,
                album_artists,
                ..
            } => (
                artists.iter().cloned().collect(),
                album.clone(),
                album_artists.iter().cloned().collect(),
            ),
            UniqueFields::Episode { show_name, .. } => {
                (Vec::new(), Some(show_name.clone()), Vec::new())
            }
        };
        Self {
            id: item.uri.rsplit(':').next().unwrap_or_default().to_owned(),
            uri: item.uri.clone(),
            title: item.name.clone(),
            artists,
            album,
            album_artists,
            length: Duration::from_millis(item.duration_ms.into()),
            art_url: item
                .covers
                .iter()
                .max_by_key(|cover| cover.width)
                .map(|cover| cover.url.clone()),
        }
    }

    /// An object path unique to the track; Spotify IDs are alphanumeric.
    fn path(&self) -> String {
        let id: String = self
            .id
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect();
        format!("{PATH}/track/{id}")
    }

    fn metadata(&self) -> Vec<(&'static str, Value<'_>)> {
        let mut fields = vec![
            (
                "mpris:trackid",
                Value::from(ObjectPath::try_from(self.path()).unwrap_or_else(|_| no_track())),
            ),
            ("mpris:length", Value::from(micros(self.length))),
            ("xesam:title", Value::from(self.title.as_str())),
            ("xesam:artist", Value::from(self.artists.clone())),
            ("xesam:albumArtist", Value::from(self.album_artists.clone())),
        ];
        if let Some(album) = &self.album {
            fields.push(("xesam:album", Value::from(album.as_str())));
        }
        if let Some(art_url) = &self.art_url {
            fields.push(("mpris:artUrl", Value::from(art_url.as_str())));
        }
        if self.uri.starts_with("spotify:track:") {
            fields.push(("xesam:url", Value::from(format!("{TRACK_URL}{}", self.id))));
        }
        fields
    }
}

/// What the player reports, kept for clients that ask.
#[derive(Debug)]
struct Now {
    track: Option<Track>,
    status: Status,
    position: Duration,
    reported: Instant,
    volume: u16,
    shuffle: bool,
    repeat: Loop,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Status {
    Stopped,
    Playing,
    Paused,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Loop {
    Off,
    Playlist,
    Track,
}

/// Which property an event changed.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Change {
    Nothing,
    Track,
    Status,
    Volume,
    Shuffle,
    Loop,
    Position(i64),
}

impl Default for Now {
    fn default() -> Self {
        Self {
            track: None,
            status: Status::Stopped,
            position: Duration::ZERO,
            reported: Instant::now(),
            volume: u16::MAX / 2,
            shuffle: false,
            repeat: Loop::Off,
        }
    }
}

impl Now {
    fn apply(&mut self, event: PlayerEvent) -> Change {
        match event {
            PlayerEvent::TrackChanged { audio_item } => {
                self.track = Some(Track::from_item(&audio_item));
                self.report(0);
                Change::Track
            }
            PlayerEvent::Playing { position_ms, .. } => self.play(Status::Playing, position_ms),
            PlayerEvent::Paused { position_ms, .. } => self.play(Status::Paused, position_ms),
            PlayerEvent::Stopped { .. } => self.play(Status::Stopped, 0),
            PlayerEvent::Seeked { position_ms, .. }
            | PlayerEvent::PositionCorrection { position_ms, .. } => {
                self.report(position_ms);
                Change::Position(micros(self.position))
            }
            PlayerEvent::PositionChanged { position_ms, .. } => {
                self.report(position_ms);
                Change::Nothing
            }
            PlayerEvent::VolumeChanged { volume } => {
                self.volume = volume;
                Change::Volume
            }
            PlayerEvent::ShuffleChanged { shuffle } => {
                self.shuffle = shuffle;
                Change::Shuffle
            }
            PlayerEvent::RepeatChanged { context, track } => {
                self.repeat = match (context, track) {
                    (_, true) => Loop::Track,
                    (true, false) => Loop::Playlist,
                    (false, false) => Loop::Off,
                };
                Change::Loop
            }
            _ => Change::Nothing,
        }
    }

    fn play(&mut self, status: Status, position_ms: u32) -> Change {
        self.status = status;
        self.report(position_ms);
        Change::Status
    }

    fn report(&mut self, position_ms: u32) {
        self.position = Duration::from_millis(position_ms.into());
        self.reported = Instant::now();
    }

    fn elapsed(&self) -> Duration {
        let since = match self.status {
            Status::Playing => self.reported.elapsed(),
            Status::Paused | Status::Stopped => Duration::ZERO,
        };
        let elapsed = self.position + since;
        match &self.track {
            Some(track) => elapsed.min(track.length),
            None => elapsed,
        }
    }
}

struct Root;

#[interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {}

    fn quit(&self) {}

    #[zbus(property)]
    fn can_quit(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn can_raise(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn identity(&self) -> String {
        DEVICE_NAME.into()
    }

    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        Vec::new()
    }

    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        Vec::new()
    }
}

struct Player {
    spirc: Arc<Spirc>,
    now: Now,
}

#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    fn next(&self) -> fdo::Result<()> {
        failed(self.spirc.next())
    }

    fn previous(&self) -> fdo::Result<()> {
        failed(self.spirc.prev())
    }

    fn pause(&self) -> fdo::Result<()> {
        failed(self.spirc.pause())
    }

    fn play_pause(&self) -> fdo::Result<()> {
        failed(self.spirc.play_pause())
    }

    fn stop(&self) -> fdo::Result<()> {
        failed(self.spirc.pause())
    }

    fn play(&self) -> fdo::Result<()> {
        failed(self.spirc.play())
    }

    fn seek(&self, offset: i64) -> fdo::Result<()> {
        let target = micros(self.now.elapsed()).saturating_add(offset).max(0);
        self.go_to(target)
    }

    #[expect(
        clippy::needless_pass_by_value,
        reason = "zbus passes method arguments by value"
    )]
    fn set_position(&self, track_id: ObjectPath<'_>, position: i64) -> fdo::Result<()> {
        // MPRIS ignores positions for a track that is no longer playing, or out of range.
        let current = self.now.track.as_ref().map(Track::path);
        let length = self
            .now
            .track
            .as_ref()
            .map_or(0, |track| micros(track.length));
        if current.as_deref() != Some(track_id.as_str()) || !(0..=length).contains(&position) {
            return Ok(());
        }
        self.go_to(position)
    }

    fn open_uri(&self, _uri: String) -> fdo::Result<()> {
        Err(fdo::Error::NotSupported(
            "spotify-tui opens tracks from its own window".into(),
        ))
    }

    #[zbus(signal)]
    async fn seeked(emitter: &SignalEmitter<'_>, position: i64) -> zbus::Result<()>;

    #[zbus(property)]
    fn playback_status(&self) -> String {
        format!("{:?}", self.now.status)
    }

    #[zbus(property)]
    fn loop_status(&self) -> String {
        match self.now.repeat {
            Loop::Off => "None".into(),
            Loop::Playlist => "Playlist".into(),
            Loop::Track => "Track".into(),
        }
    }

    #[expect(
        clippy::needless_pass_by_value,
        reason = "zbus passes property values by value"
    )]
    #[zbus(property)]
    fn set_loop_status(&self, status: String) -> fdo::Result<()> {
        let (context, track) = match status.as_str() {
            "None" => (false, false),
            "Playlist" => (true, false),
            "Track" => (true, true),
            _ => {
                return Err(fdo::Error::InvalidArgs(format!(
                    "unknown loop status {status}"
                )));
            }
        };
        failed(
            self.spirc
                .repeat(context)
                .and_then(|()| self.spirc.repeat_track(track)),
        )
    }

    #[zbus(property)]
    fn shuffle(&self) -> bool {
        self.now.shuffle
    }

    #[zbus(property)]
    fn set_shuffle(&self, shuffle: bool) -> fdo::Result<()> {
        failed(self.spirc.shuffle(shuffle))
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        let Some(track) = &self.now.track else {
            return [("mpris:trackid", Value::from(no_track()))]
                .into_iter()
                .filter_map(owned)
                .collect();
        };
        track.metadata().into_iter().filter_map(owned).collect()
    }

    #[zbus(property)]
    fn volume(&self) -> f64 {
        f64::from(self.now.volume) / f64::from(u16::MAX)
    }

    #[zbus(property)]
    fn set_volume(&self, volume: f64) -> fdo::Result<()> {
        // Clamped to 0–1 first, so the conversion cannot overflow.
        let volume = (volume.clamp(0.0, 1.0) * f64::from(u16::MAX)).round() as u16;
        failed(self.spirc.set_volume(volume))
    }

    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        micros(self.now.elapsed())
    }

    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_play(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_pause(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_seek(&self) -> bool {
        self.now.track.is_some()
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn can_control(&self) -> bool {
        true
    }
}

impl Player {
    fn go_to(&self, position: i64) -> fdo::Result<()> {
        // Positions past u32 milliseconds (49 days) clamp to the end of the track.
        let millis = (position / MICROS_PER_MILLI).try_into().unwrap_or(u32::MAX);
        failed(self.spirc.set_position_ms(millis))
    }
}

const MICROS_PER_MILLI: i64 = 1000;

fn micros(duration: Duration) -> i64 {
    // Saturates only past 292 thousand years.
    duration.as_micros().try_into().unwrap_or(i64::MAX)
}

fn no_track() -> ObjectPath<'static> {
    ObjectPath::from_static_str_unchecked(NO_TRACK)
}

fn owned((key, value): (&str, Value<'_>)) -> Option<(String, OwnedValue)> {
    // Only values carrying file descriptors fail to convert; metadata has none.
    Some((key.to_owned(), value.try_into().ok()?))
}

fn failed(result: Result<(), librespot_core::Error>) -> fdo::Result<()> {
    result.map_err(|error| fdo::Error::Failed(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{Change, Loop, Now, Status, Track};
    use librespot_playback::player::PlayerEvent;
    use std::time::Duration;

    fn track() -> Track {
        Track {
            id: "4uLU6hMCjMI75M1A2tKUQC".into(),
            uri: "spotify:track:4uLU6hMCjMI75M1A2tKUQC".into(),
            title: "Never Gonna Give You Up".into(),
            artists: vec!["Rick Astley".into()],
            album: Some("Whenever You Need Somebody".into()),
            album_artists: vec!["Rick Astley".into()],
            length: Duration::from_millis(213_573),
            art_url: None,
        }
    }

    #[test]
    fn tracks_have_valid_object_paths() {
        assert_eq!(
            track().path(),
            "/org/mpris/MediaPlayer2/track/4uLU6hMCjMI75M1A2tKUQC"
        );
        let track = track();
        let metadata = track.metadata();
        let keys: Vec<_> = metadata.iter().map(|(key, _)| *key).collect();
        assert!(keys.contains(&"mpris:trackid"));
        assert!(keys.contains(&"xesam:url"));
    }

    #[test]
    fn events_update_the_reported_state() {
        let mut now = Now {
            track: Some(track()),
            ..Now::default()
        };
        let playing = PlayerEvent::Playing {
            play_request_id: 0,
            track_id: librespot_core::SpotifyUri::from_uri(&track().uri)
                .expect("the test track URI is valid"),
            position_ms: 1000,
        };
        assert_eq!(now.apply(playing), Change::Status);
        assert_eq!(now.status, Status::Playing);
        assert!(now.elapsed() >= Duration::from_secs(1));
        let repeat = PlayerEvent::RepeatChanged {
            context: true,
            track: true,
        };
        assert_eq!(now.apply(repeat), Change::Loop);
        assert_eq!(now.repeat, Loop::Track);
        assert_eq!(
            now.apply(PlayerEvent::VolumeChanged { volume: 7 }),
            Change::Volume
        );
        assert_eq!(now.volume, 7);
    }
}
