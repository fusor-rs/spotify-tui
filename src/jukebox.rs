mod browse;
mod keys;
mod listing;
mod playback;
mod session;
mod view;

use crate::spotify::{self, Catalog, Playlist, Speaker};
use fusor::{FromInputs, OwnerHandle, Signal, signal};
use hypercmd::{Error, Node, Services};
use listing::{Listing, Source, Window};
use playback::NowPlaying;
use std::{
    cell::{Cell, RefCell},
    future::Future,
    rc::Rc,
};

/// How long a notice stays on screen, in clock ticks.
const NOTICE_TICKS: usize = 16;

pub(crate) struct Startup {
    pub(crate) http: reqwest::Client,
    pub(crate) refresh_token: Option<String>,
}

#[derive(Clone, PartialEq)]
pub(crate) enum Screen {
    Connecting,
    LoggedOut,
    Waiting(String),
    Failed(String),
    Library,
}

#[derive(Clone, Copy, PartialEq)]
enum Tone {
    Info,
    Error,
}

#[derive(Clone, PartialEq)]
struct Notice {
    text: String,
    tone: Tone,
    shown: usize,
}

pub(crate) struct Jukebox {
    owner: OwnerHandle,
    services: Services,
    http: reqwest::Client,
    speaker: RefCell<Option<Speaker>>,
    catalog: RefCell<Option<Catalog>>,
    screen: Signal<Screen>,
    playlists: Signal<Vec<Rc<Playlist>>>,
    listing: Signal<Listing>,
    window: Signal<Window>,
    history: RefCell<Vec<(Listing, Window)>>,
    generation: Cell<u64>,
    search: Signal<String>,
    now: Signal<NowPlaying>,
    clock: Signal<usize>,
    bar_width: Signal<usize>,
    help: Signal<bool>,
    notice: Signal<Option<Notice>>,
    root: RefCell<Option<Node>>,
    saved_login: RefCell<Option<String>>,
}

impl FromInputs for Jukebox {
    type Inputs = Startup;
    type Error = Error;

    fn from_inputs(startup: Startup, owner: OwnerHandle) -> Result<Self, Error> {
        let screen = match startup.refresh_token {
            Some(_) => Screen::Connecting,
            None => Screen::LoggedOut,
        };
        Ok(Self {
            services: Services::from_owner(&owner)?,
            owner,
            http: startup.http,
            speaker: RefCell::new(None),
            catalog: RefCell::new(None),
            screen: signal(screen),
            playlists: signal(Vec::new()),
            listing: signal(Listing::new(Source::Liked)),
            window: signal(Window::default()),
            history: RefCell::new(Vec::new()),
            generation: Cell::new(0),
            search: signal(String::new()),
            now: signal(NowPlaying::default()),
            clock: signal(0),
            bar_width: signal(0),
            help: signal(false),
            notice: signal(None),
            root: RefCell::new(None),
            saved_login: RefCell::new(startup.refresh_token),
        })
    }
}

impl Jukebox {
    fn catalog(&self) -> Option<Catalog> {
        self.catalog.borrow().clone()
    }

    /// Runs Spotify work in the background and reports its failure as a notice.
    fn spawn(
        &self,
        task: impl Future<Output = Result<(), spotify::Error>> + 'static,
    ) -> Result<(), Error> {
        let notice = self.notice.clone();
        let clock = self.clock.clone();
        self.services.spawn(&self.owner, async move {
            if let Err(error) = task.await {
                notify(&notice, &clock, error.to_string(), Tone::Error);
            }
        })
    }

    fn inform(&self, text: impl Into<String>) {
        notify(&self.notice, &self.clock, text.into(), Tone::Info);
    }

    fn warn(&self, text: String) {
        notify(&self.notice, &self.clock, text, Tone::Error);
    }

    fn notice_text(&self) -> String {
        let now = self.clock.get();
        self.notice.with(|notice| {
            notice
                .as_ref()
                .filter(|notice| now.saturating_sub(notice.shown) < NOTICE_TICKS)
                .map_or_else(String::new, |notice| notice.text.clone())
        })
    }

    fn notice_is_error(&self) -> bool {
        self.notice.with(|notice| {
            notice
                .as_ref()
                .is_some_and(|notice| notice.tone == Tone::Error)
        })
    }

    fn focus(&self, id: &str) {
        if let Some(node) = self.root.borrow().as_ref().and_then(|root| root.find(id)) {
            node.request_focus();
        }
    }
}

fn notify(notice: &Signal<Option<Notice>>, clock: &Signal<usize>, text: String, tone: Tone) {
    notice.set(Some(Notice {
        text,
        tone,
        shown: clock.get_untracked(),
    }));
}
