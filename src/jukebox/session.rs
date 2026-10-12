use super::{Jukebox, Screen, listing::Source};
use crate::spotify::{self, Authorization, INITIAL_VOLUME, Speaker, Token};
use hypercmd::{Error, Event};
use std::{
    process::{Command, Stdio},
    rc::Rc,
    time::Duration,
};
use tokio::sync::mpsc;

#[cfg(target_os = "macos")]
const BROWSER_OPENER: &str = "open";
#[cfg(not(target_os = "macos"))]
const BROWSER_OPENER: &str = "xdg-open";
/// How long to wait before trying again to reach Spotify after a lost session.
const RECONNECT_DELAY: Duration = Duration::from_secs(5);

impl Jukebox {
    /// Captures the root on first presentation and resumes a saved login.
    pub(super) fn attach(self: &Rc<Self>, event: &Event) -> Result<(), Error> {
        if self.root.replace(Some(event.target.clone())).is_some() {
            return Ok(());
        }
        let Some(refresh_token) = self.saved_login.take() else {
            return Ok(());
        };
        let jukebox = self.clone();
        self.start(async move {
            let token = Token::refresh(&jukebox.http, &refresh_token).await?;
            token.store()?;
            jukebox.connect(token).await
        })
    }

    pub(super) fn log_in(self: &Rc<Self>) -> Result<(), Error> {
        let authorization = Authorization::new();
        // The page link stays on screen, so a browser that fails to open is not fatal.
        let _ = Command::new(BROWSER_OPENER)
            .arg(&authorization.url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        self.screen.set(Screen::Waiting(authorization.url.clone()));
        let jukebox = self.clone();
        self.start(async move {
            let token = authorization.complete(&jukebox.http).await?;
            token.store()?;
            jukebox.connect(token).await
        })
    }

    /// Runs a login step, showing its failure on the welcome screen.
    fn start(
        &self,
        task: impl Future<Output = Result<(), spotify::Error>> + 'static,
    ) -> Result<(), Error> {
        let screen = self.screen.clone();
        self.services.spawn(&self.owner, async move {
            if let Err(error) = task.await {
                screen.set(Screen::Failed(error.to_string()));
            }
        })
    }

    async fn connect(self: Rc<Self>, token: Token) -> Result<(), spotify::Error> {
        self.screen.set(Screen::Connecting);
        let (events, player_events) = mpsc::unbounded_channel();
        let connection = Speaker::connect(token.access, events.clone(), INITIAL_VOLUME).await?;
        let lost = self.install(connection);
        self.screen.set(Screen::Library);
        for started in [self.follow_player(player_events), self.start_clock()] {
            if let Err(error) = started {
                self.warn(format!("Playback updates stopped: {error}"));
            }
        }
        let watch = self.clone().stay_connected(lost, events);
        if let Err(error) = self.services.spawn(&self.owner, watch) {
            self.warn(format!("Cannot watch the connection to Spotify: {error}"));
        }
        self.load_playlists();
        self.open(Source::Liked);
        self.focus("tracks");
        Ok(())
    }

    fn install(&self, connection: spotify::Connection) -> spotify::Lost {
        self.speaker.replace(Some(connection.speaker));
        self.catalog.replace(Some(connection.catalog));
        connection.lost
    }

    /// librespot drops its session after a sleep or a network change and never
    /// reconnects, so a new speaker and catalog replace the dead ones.
    async fn stay_connected(self: Rc<Self>, mut lost: spotify::Lost, events: spotify::Events) {
        // A speaker dropped on purpose ends the watch without a loss.
        while lost.await.is_ok() {
            // Stopping the dead speaker releases its audio output and MPRIS name.
            let volume = self
                .speaker
                .replace(None)
                .map_or(INITIAL_VOLUME, |speaker| speaker.volume());
            self.inform("Lost the connection to Spotify; reconnecting…");
            let Some(next) = self.reconnect_until_done(&events, volume).await else {
                return;
            };
            lost = next;
            self.inform("Reconnected to Spotify");
        }
    }

    /// Retries until a new speaker is up, or until retrying cannot help.
    async fn reconnect_until_done(
        &self,
        events: &spotify::Events,
        volume: u16,
    ) -> Option<spotify::Lost> {
        loop {
            match self.reconnect(events, volume).await {
                Ok(lost) => return Some(lost),
                Err(error @ spotify::Error::Unauthorized) => {
                    self.warn(error.to_string());
                    return None;
                }
                Err(error) => self.warn(format!("Cannot reconnect to Spotify yet: {error}")),
            }
            self.services.sleep(RECONNECT_DELAY).ok()?.await.ok()?;
        }
    }

    async fn reconnect(
        &self,
        events: &spotify::Events,
        volume: u16,
    ) -> Result<spotify::Lost, spotify::Error> {
        let refresh_token = spotify::stored_refresh_token()?.ok_or(spotify::Error::Unauthorized)?;
        let token = Token::refresh(&self.http, &refresh_token).await?;
        token.store()?;
        let connection = Speaker::connect(token.access, events.clone(), volume).await?;
        Ok(self.install(connection))
    }

    pub(super) fn login_url(&self) -> String {
        match self.screen.get() {
            Screen::Waiting(url) => url,
            _ => String::new(),
        }
    }

    pub(super) fn welcome_status(&self) -> String {
        match self.screen.get() {
            Screen::Connecting => "Connecting to Spotify…".into(),
            Screen::LoggedOut => "Log in once; your login is kept in the system keychain.".into(),
            Screen::Waiting(_) => {
                "Approve spotify-tui in your browser. This screen continues by itself.".into()
            }
            Screen::Failed(error) => error,
            Screen::Library => String::new(),
        }
    }
}
