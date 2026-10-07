use super::{Jukebox, Screen, listing::Source};
use crate::spotify::{self, Authorization, Speaker, Token};
use hypercmd::{Error, Event};
use std::{
    process::{Command, Stdio},
    rc::Rc,
};

#[cfg(target_os = "macos")]
const BROWSER_OPENER: &str = "open";
#[cfg(not(target_os = "macos"))]
const BROWSER_OPENER: &str = "xdg-open";

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
        let connection = Speaker::connect(token.access).await?;
        self.speaker.replace(Some(connection.speaker));
        self.catalog.replace(Some(connection.catalog));
        self.screen.set(Screen::Library);
        for started in [self.follow_player(connection.events), self.start_clock()] {
            if let Err(error) = started {
                self.warn(format!("Playback updates stopped: {error}"));
            }
        }
        self.load_playlists();
        self.open(Source::Liked);
        self.focus("tracks");
        Ok(())
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
