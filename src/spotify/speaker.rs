use super::{Catalog, Error, Repeat};
use http::{HeaderMap, HeaderValue, Method, header::CONTENT_TYPE};
use librespot_connect::{
    ConnectConfig, LoadContextOptions, LoadRequest, LoadRequestOptions, Options, PlayingTrack,
    Spirc,
};
use librespot_core::{Session, SessionConfig, authentication::Credentials, config::DeviceType};
use librespot_playback::{
    audio_backend,
    config::{AudioFormat, PlayerConfig},
    mixer::{self, Mixer, MixerConfig},
    player::{Player, PlayerEvent, PlayerEventChannel},
};
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

/// The name this computer shows in every Spotify Connect device list.
pub(crate) const DEVICE_NAME: &str = "spotify-tui";
/// Full volume: the system mixer sets the loudness. librespot's volume curve is
/// logarithmic over 60 dB, so half of the range already plays 30 dB quieter.
pub(crate) const INITIAL_VOLUME: u16 = u16::MAX;
/// How often a speaker checks that librespot still considers its session valid.
const SESSION_CHECK: Duration = Duration::from_secs(1);

/// Where speakers send their player events; one channel outlives every reconnection.
pub(crate) type Events = mpsc::UnboundedSender<PlayerEvent>;
/// Resolves once the speaker's session is lost; fails if the speaker was dropped first.
pub(crate) type Lost = oneshot::Receiver<()>;

/// A Spotify Connect speaker that plays audio on this computer.
pub(crate) struct Speaker {
    spirc: Arc<Spirc>,
    session: Session,
    mixer: Arc<dyn Mixer>,
    /// Dropped with the speaker, so that stopping it on purpose is not reported as lost.
    _watch: oneshot::Sender<()>,
}

/// A logged-in session: the speaker, the catalog and the news of their session's loss.
pub(crate) struct Connection {
    pub(crate) speaker: Speaker,
    pub(crate) catalog: Catalog,
    pub(crate) lost: Lost,
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
    /// Logs in and registers the speaker, which starts at `volume` and sends its player
    /// events to `events`. Must run inside the Tokio runtime.
    pub(crate) async fn connect(
        access_token: String,
        events: Events,
        volume: u16,
    ) -> Result<Connection, Error> {
        tokio::spawn(start(access_token, events, volume)).await?
    }

    /// The volume the speaker plays at, which a replacement speaker keeps.
    pub(crate) fn volume(&self) -> u16 {
        self.mixer.volume()
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

    /// Adds a track after the playing one and any tracks queued before it.
    pub(crate) fn queue(&self, track: String) -> impl Future<Output = Result<(), Error>> + 'static {
        let session = self.session.clone();
        async move { tokio::spawn(add_to_queue(session, track)).await? }
    }
}

impl Drop for Speaker {
    fn drop(&mut self) {
        // The process is exiting; a speaker that already stopped needs no shutdown.
        let _ = self.spirc.shutdown();
    }
}

async fn start(access_token: String, events: Events, volume: u16) -> Result<Connection, Error> {
    let backend = audio_backend::find(None).ok_or(Error::NoAudio)?;
    let mixer = mixer::find(None).ok_or(Error::NoAudio)?(MixerConfig::default())?;
    let session = Session::new(SessionConfig::default(), None);
    let player = Player::new(
        PlayerConfig::default(),
        session.clone(),
        mixer.get_soft_volume(),
        move || backend(None, AudioFormat::default()),
    );
    tokio::spawn(forward(player.get_player_event_channel(), events));
    #[cfg(target_os = "linux")]
    let media_events = player.get_player_event_channel();
    let config = ConnectConfig {
        name: DEVICE_NAME.into(),
        device_type: DeviceType::Computer,
        initial_volume: volume,
        ..ConnectConfig::default()
    };
    let credentials = Credentials::with_access_token(access_token);
    let (spirc, task) =
        Spirc::new(config, session.clone(), credentials, player, mixer.clone()).await?;
    let task = tokio::spawn(task);
    let (watch, stopped) = oneshot::channel();
    let (report, lost) = oneshot::channel();
    tokio::spawn(watch_session(session.clone(), task, stopped, report));
    let spirc = Arc::new(spirc);
    #[cfg(target_os = "linux")]
    super::mpris::publish(spirc.clone(), media_events);
    Ok(Connection {
        speaker: Speaker {
            spirc,
            session: session.clone(),
            mixer,
            _watch: watch,
        },
        catalog: Catalog::new(session),
        lost,
    })
}

/// Passes a player's events on until the player or the listener is gone.
async fn forward(mut from: PlayerEventChannel, to: Events) {
    while let Some(event) = from.recv().await {
        if to.send(event).is_err() {
            return;
        }
    }
}

/// Reports the session lost once Spirc stops by itself or librespot invalidates the
/// session, as its keep-alive does after a sleep or a network change. librespot never
/// reconnects, and an invalidated session cannot be reused.
async fn watch_session(
    session: Session,
    task: JoinHandle<()>,
    mut stopped: oneshot::Receiver<()>,
    report: oneshot::Sender<()>,
) {
    let mut checks = tokio::time::interval(SESSION_CHECK);
    loop {
        checks.tick().await;
        if !matches!(stopped.try_recv(), Err(oneshot::error::TryRecvError::Empty)) {
            return;
        }
        if task.is_finished() || session.is_invalid() {
            break;
        }
    }
    // Nobody listens any more once the app is gone.
    let _ = report.send(());
}

/// Asks Spotify to send this speaker an add-to-queue command, as other Spotify apps do.
async fn add_to_queue(session: Session, track: String) -> Result<(), Error> {
    let device = session.device_id();
    let endpoint = format!("/connect-state/v1/player/command/from/{device}/to/{device}");
    let body = serde_json::json!({
        "command": {
            "endpoint": "add_to_queue",
            "track": { "uri": track, "provider": "queue", "metadata": { "is_queued": "true" } },
            "logging_params": {},
        },
    });
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    session
        .spclient()
        .request(
            &Method::POST,
            &endpoint,
            Some(headers),
            Some(body.to_string().as_bytes()),
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::watch_session;
    use librespot_core::{Session, SessionConfig};
    use tokio::sync::oneshot;

    fn block_on<T>(future: impl Future<Output = T>) -> T {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        runtime.block_on(future)
    }

    /// Watches a Spirc task that runs until `finish` is sent or dropped.
    fn watch(session: &Session) -> (oneshot::Sender<()>, oneshot::Sender<()>, super::Lost) {
        let (finish, finished) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let _ = finished.await;
        });
        let (keep, stopped) = oneshot::channel();
        let (report, lost) = oneshot::channel();
        tokio::spawn(watch_session(session.clone(), task, stopped, report));
        (finish, keep, lost)
    }

    #[test]
    fn an_invalidated_session_is_reported_lost() {
        block_on(async {
            let session = Session::new(SessionConfig::default(), None);
            let (_finish, _keep, lost) = watch(&session);
            session.shutdown();
            assert!(lost.await.is_ok());
        });
    }

    #[test]
    fn a_spirc_that_stops_by_itself_is_reported_lost() {
        block_on(async {
            let session = Session::new(SessionConfig::default(), None);
            let (finish, _keep, lost) = watch(&session);
            drop(finish);
            assert!(lost.await.is_ok());
        });
    }

    #[test]
    fn a_speaker_stopped_on_purpose_is_not_reported_lost() {
        block_on(async {
            let session = Session::new(SessionConfig::default(), None);
            let (finish, keep, lost) = watch(&session);
            drop(keep);
            drop(finish);
            assert!(lost.await.is_err());
        });
    }
}
