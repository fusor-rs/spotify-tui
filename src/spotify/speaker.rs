use super::{Catalog, Error, Repeat};
use librespot_connect::{
    ConnectConfig, LoadContextOptions, LoadRequest, LoadRequestOptions, Options, PlayingTrack,
    Spirc,
};
use librespot_core::{Session, SessionConfig, authentication::Credentials, config::DeviceType};
use librespot_playback::{
    audio_backend,
    config::{AudioFormat, PlayerConfig},
    mixer::{self, MixerConfig},
    player::{Player, PlayerEventChannel},
};

/// The name this computer shows in every Spotify Connect device list.
pub(crate) const DEVICE_NAME: &str = "spotify-tui";
const INITIAL_VOLUME: u16 = u16::MAX / 2;

/// A Spotify Connect speaker that plays audio on this computer.
pub(crate) struct Speaker {
    spirc: Spirc,
}

/// A logged-in session: the speaker, its player events and the catalog.
pub(crate) struct Connection {
    pub(crate) speaker: Speaker,
    pub(crate) events: PlayerEventChannel,
    pub(crate) catalog: Catalog,
}

/// What to start playing: a track within a context, or within a list of tracks.
pub(crate) enum Play {
    Context { uri: String, track: String },
    Tracks { uris: Vec<String>, track: String },
}

/// The shuffle and repeat a newly played list starts with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Mode {
    pub(crate) shuffle: bool,
    pub(crate) repeat: Repeat,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Command {
    Resume,
    Pause,
    Next,
    Previous,
    Seek(u32),
    Volume(u16),
    Shuffle(bool),
    Repeat(Repeat),
}

impl Speaker {
    /// Logs in and registers the speaker. Must run inside the Tokio runtime.
    pub(crate) async fn connect(access_token: String) -> Result<Connection, Error> {
        tokio::spawn(start(access_token)).await?
    }

    pub(crate) fn play(&self, play: Play, mode: Mode) -> Result<(), Error> {
        let options = |track| LoadRequestOptions {
            start_playing: true,
            playing_track: Some(PlayingTrack::Uri(track)),
            context_options: Some(LoadContextOptions::Options(Options {
                shuffle: mode.shuffle,
                repeat: mode.repeat != Repeat::Off,
                repeat_track: mode.repeat == Repeat::Track,
            })),
            ..LoadRequestOptions::default()
        };
        let request = match play {
            Play::Context { uri, track } => LoadRequest::from_context_uri(uri, options(track)),
            Play::Tracks { uris, track } => LoadRequest::from_tracks(uris, options(track)),
        };
        // A speaker ignores commands until it is the active device; activating it again is harmless.
        self.spirc.activate()?;
        Ok(self.spirc.load(request)?)
    }

    pub(crate) fn command(&self, command: Command) -> Result<(), Error> {
        match command {
            Command::Resume => self.spirc.play(),
            Command::Pause => self.spirc.pause(),
            Command::Next => self.spirc.next(),
            Command::Previous => self.spirc.prev(),
            Command::Seek(position) => self.spirc.set_position_ms(position),
            Command::Volume(volume) => self.spirc.set_volume(volume),
            Command::Shuffle(enabled) => self.spirc.shuffle(enabled),
            Command::Repeat(mode) => self
                .spirc
                .repeat(mode != Repeat::Off)
                .and_then(|()| self.spirc.repeat_track(mode == Repeat::Track)),
        }?;
        Ok(())
    }
}

impl Drop for Speaker {
    fn drop(&mut self) {
        // The process is exiting; a speaker that already stopped needs no shutdown.
        let _ = self.spirc.shutdown();
    }
}

async fn start(access_token: String) -> Result<Connection, Error> {
    let backend = audio_backend::find(None).ok_or(Error::NoAudio)?;
    let mixer = mixer::find(None).ok_or(Error::NoAudio)?(MixerConfig::default())?;
    let session = Session::new(SessionConfig::default(), None);
    let player = Player::new(
        PlayerConfig::default(),
        session.clone(),
        mixer.get_soft_volume(),
        move || backend(None, AudioFormat::default()),
    );
    let events = player.get_player_event_channel();
    let config = ConnectConfig {
        name: DEVICE_NAME.into(),
        device_type: DeviceType::Computer,
        initial_volume: INITIAL_VOLUME,
        ..ConnectConfig::default()
    };
    let credentials = Credentials::with_access_token(access_token);
    let (spirc, task) = Spirc::new(config, session.clone(), credentials, player, mixer).await?;
    tokio::spawn(task);
    Ok(Connection {
        speaker: Speaker { spirc },
        events,
        catalog: Catalog::new(session),
    })
}
