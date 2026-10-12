use super::{Jukebox, listing::duration};
use crate::{
    cover,
    spotify::{Command, DEVICE_NAME, Error as SpotifyError, Mode, Repeat, Speaker, Track},
};
use hypercmd::{Error, Event, EventPayload};
use librespot_playback::player::{PlayerEvent, PlayerEventChannel};
use std::{
    rc::Rc,
    time::{Duration, Instant},
};

const TICK: Duration = Duration::from_millis(250);
pub(super) const SEEK_STEP: Duration = Duration::from_secs(10);
/// One step of the volume keys: 5% of the speaker's range.
const VOLUME_STEP: u16 = u16::MAX / 20;
const VOLUME_CELLS: u32 = 10;
const PERCENT: u32 = 100;
const EQUALIZER_LEVELS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
const EQUALIZER_PATTERN: [usize; 12] = [2, 5, 7, 4, 6, 3, 1, 4, 7, 5, 2, 6];
const EQUALIZER_BARS: usize = 4;
const EQUALIZER_SPREAD: usize = 5;

/// What the speaker reports it is playing.
#[derive(Clone, PartialEq)]
pub(super) struct NowPlaying {
    uri: Option<String>,
    track: Option<Rc<Track>>,
    playing: bool,
    /// librespot ignores Resume once stopped, after the end of a list or `p` on its first track.
    stopped: bool,
    position: Duration,
    reported: Instant,
    volume: u16,
    shuffle: bool,
    repeat: Repeat,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum VolumeStep {
    Up,
    Down,
}

impl Default for NowPlaying {
    fn default() -> Self {
        Self {
            uri: None,
            track: None,
            playing: false,
            stopped: false,
            position: Duration::ZERO,
            reported: Instant::now(),
            volume: crate::spotify::INITIAL_VOLUME,
            shuffle: false,
            repeat: Repeat::Off,
        }
    }
}

impl NowPlaying {
    fn report(&mut self, position_ms: u32, playing: bool) {
        self.position = Duration::from_millis(position_ms.into());
        self.reported = Instant::now();
        self.playing = playing;
        self.stopped = false;
    }

    /// Adds the details of `uri`, unless another track started meanwhile.
    fn describe(&mut self, uri: &str, track: Option<Track>) {
        if self.uri.as_deref() == Some(uri) {
            self.track = track.map(Rc::new);
        }
    }

    fn elapsed(&self) -> Duration {
        let since = if self.playing {
            self.reported.elapsed()
        } else {
            Duration::ZERO
        };
        let elapsed = self.position + since;
        match &self.track {
            Some(track) => elapsed.min(Duration::from_millis(track.duration_ms)),
            None => elapsed,
        }
    }
}

impl Jukebox {
    pub(super) fn start_clock(self: &Rc<Self>) -> Result<(), Error> {
        self.services.spawn(&self.owner, self.clone().run_clock())
    }

    async fn run_clock(self: Rc<Self>) {
        // The clock stops when the app is disposed or out of timers.
        while let Ok(sleep) = self.services.sleep(TICK) {
            if sleep.await.is_err() {
                return;
            }
            self.clock.update(|clock| *clock += 1);
        }
    }

    pub(super) fn follow_player(self: &Rc<Self>, events: PlayerEventChannel) -> Result<(), Error> {
        self.services
            .spawn(&self.owner, self.clone().run_player(events))
    }

    async fn run_player(self: Rc<Self>, mut events: PlayerEventChannel) {
        while let Some(event) = events.recv().await {
            self.apply(event);
        }
        self.warn("The speaker stopped. Restart spotify-tui to play again.".into());
    }

    fn apply(self: &Rc<Self>, event: PlayerEvent) {
        match event {
            PlayerEvent::TrackChanged { audio_item } => {
                let covers = audio_item.covers.iter();
                let cover = cover::pick(covers.map(|image| (image.width, image.url.as_str())));
                self.change_track(audio_item.uri, cover);
            }
            PlayerEvent::Playing { position_ms, .. } => {
                self.now.update(|now| now.report(position_ms, true));
            }
            PlayerEvent::Paused { position_ms, .. } => {
                self.now.update(|now| now.report(position_ms, false));
            }
            PlayerEvent::Seeked { position_ms, .. }
            | PlayerEvent::PositionCorrection { position_ms, .. }
            | PlayerEvent::PositionChanged { position_ms, .. } => {
                self.now.update(|now| now.report(position_ms, now.playing));
            }
            PlayerEvent::Stopped { .. } => self.now.update(|now| {
                now.playing = false;
                now.stopped = true;
            }),
            PlayerEvent::VolumeChanged { volume } => self.now.update(|now| now.volume = volume),
            PlayerEvent::ShuffleChanged { shuffle } => self.now.update(|now| now.shuffle = shuffle),
            PlayerEvent::RepeatChanged { context, track } => self.now.update(|now| {
                now.repeat = match (context, track) {
                    (_, true) => Repeat::Track,
                    (true, false) => Repeat::Context,
                    (false, false) => Repeat::Off,
                };
            }),
            PlayerEvent::Unavailable { .. } => {
                self.warn("Spotify can't play this track here; skipping it.".into());
            }
            _ => {}
        }
    }

    fn change_track(self: &Rc<Self>, uri: String, cover: Option<String>) {
        self.now.update(|now| {
            now.uri = Some(uri.clone());
            now.track = None;
            now.stopped = false;
        });
        self.change_cover(&uri, cover);
        let Some(catalog) = self.catalog() else {
            return;
        };
        let now = self.now.clone();
        let started = self.spawn(async move {
            let track = catalog.tracks(vec![uri.clone()]).await?.into_iter().next();
            now.update(|now| now.describe(&uri, track));
            Ok(())
        });
        if let Err(error) = started {
            self.warn(error.to_string());
        }
    }

    /// Shows the cover of `uri`; an album keeps its cover from one track to the next.
    fn change_cover(&self, uri: &str, url: Option<String>) {
        if !cover::supported() || self.cover.with_untracked(|shown| *shown == url) {
            return;
        }
        self.cover.set(None);
        let Some(url) = url else {
            // Nothing shows the old cover any more; the terminal can free it.
            if let Err(error) = cover::clear() {
                self.warn(format!("Cannot read the album cover ({error})"));
            }
            return;
        };
        let (http, now, shown, uri) = (
            self.http.clone(),
            self.now.clone(),
            self.cover.clone(),
            uri.to_owned(),
        );
        let started = self.spawn(async move {
            let pixels = cover::fetch(http, url.clone()).await?;
            // Another track may have started during the download.
            if now.with_untracked(|now| now.uri.as_deref() == Some(uri.as_str())) {
                cover::show(&pixels).map_err(|error| SpotifyError::Cover(error.to_string()))?;
                shown.set(Some(url));
            }
            Ok(())
        });
        if let Err(error) = started {
            self.warn(error.to_string());
        }
    }

    pub(super) fn with_speaker(
        &self,
        action: impl FnOnce(&Speaker) -> Result<(), crate::spotify::Error>,
    ) {
        let result = match self.speaker.borrow().as_ref() {
            Some(speaker) => action(speaker),
            None => return,
        };
        if let Err(error) = result {
            self.warn(error.to_string());
        }
    }

    pub(super) fn command(&self, command: Command) {
        self.with_speaker(|speaker| speaker.command(command));
    }

    pub(super) fn toggle_play(&self) -> Result<(), Error> {
        let (started, playing) = self
            .now
            .with_untracked(|now| (now.uri.is_some() && !now.stopped, now.playing));
        if !started {
            return self.play_selected();
        }
        self.command(if playing {
            Command::Pause
        } else {
            Command::Resume
        });
        Ok(())
    }

    pub(super) fn seek_forward(&self) {
        // librespot ignores a seek past the end, so the last seconds would not skip.
        let target = self.now.with_untracked(|now| {
            let target = now.elapsed() + SEEK_STEP;
            match &now.track {
                Some(track) => target.min(Duration::from_millis(track.duration_ms)),
                None => target,
            }
        });
        self.seek(target);
    }

    pub(super) fn seek_back(&self) {
        let target = self
            .now
            .with_untracked(|now| now.elapsed().saturating_sub(SEEK_STEP));
        self.seek(target);
    }

    fn seek(&self, target: Duration) {
        // Positions past u32 milliseconds (49 days) clamp to the end of the track.
        let position = target.as_millis().try_into().unwrap_or(u32::MAX);
        self.command(Command::Seek(position));
    }

    pub(super) fn change_volume(&self, step: VolumeStep) {
        let volume = self.now.with_untracked(|now| match step {
            VolumeStep::Up => now.volume.saturating_add(VOLUME_STEP),
            VolumeStep::Down => now.volume.saturating_sub(VOLUME_STEP),
        });
        self.command(Command::Volume(volume));
    }

    pub(super) fn toggle_shuffle(&self) {
        let shuffle = self.now.with_untracked(|now| !now.shuffle);
        self.command(Command::Shuffle(shuffle));
    }

    pub(super) fn cycle_repeat(&self) {
        let repeat = self.now.with_untracked(|now| now.repeat.next());
        self.command(Command::Repeat(repeat));
    }

    pub(super) fn mode(&self) -> Mode {
        self.now.with_untracked(|now| Mode {
            shuffle: now.shuffle,
            repeat: now.repeat,
        })
    }

    pub(super) fn playing_uri(&self) -> Option<String> {
        self.now.with(|now| now.uri.clone())
    }
}

/// Text and state shown in the now-playing bar.
impl Jukebox {
    pub(super) fn is_playing(&self) -> bool {
        self.now.with(|now| now.playing)
    }

    pub(super) fn cover_shown(&self) -> bool {
        cover::supported()
    }

    /// One row of the cover, or blank cells until the terminal has the image.
    pub(super) fn cover_row(&self, row: usize) -> String {
        if self.cover.with(Option::is_some) {
            cover::row(row)
        } else {
            " ".repeat(cover::COLUMNS)
        }
    }

    pub(super) fn track_title(&self) -> String {
        self.now.with(|now| match (&now.track, &now.uri) {
            (Some(track), _) => track.name.clone(),
            (None, Some(_)) => "Loading…".into(),
            (None, None) => "Nothing playing".into(),
        })
    }

    pub(super) fn track_details(&self) -> String {
        self.now.with(|now| match &now.track {
            Some(track) => format!("{} · {}", track.artist_names(), track.album.name),
            None => format!("Choose a track and press Enter · plays here as “{DEVICE_NAME}”"),
        })
    }

    fn elapsed(&self) -> Duration {
        self.clock.get();
        self.now.with(NowPlaying::elapsed)
    }

    fn length(&self) -> Duration {
        self.now.with(|now| {
            now.track.as_ref().map_or(Duration::ZERO, |track| {
                Duration::from_millis(track.duration_ms)
            })
        })
    }

    pub(super) fn elapsed_label(&self) -> String {
        label(self.elapsed())
    }

    pub(super) fn length_label(&self) -> String {
        label(self.length())
    }

    /// Cells of the progress bar that are played, including the playhead.
    fn played_cells(&self) -> usize {
        let width = self.bar_width.get();
        let length = self.length();
        if length.is_zero() || width == 0 {
            return 0;
        }
        let fraction = self.elapsed().as_secs_f64() / length.as_secs_f64();
        ((fraction * width as f64).round() as usize).clamp(1, width)
    }

    pub(super) fn progress_played(&self) -> String {
        match self.played_cells() {
            0 => String::new(),
            cells => format!("{}●", "━".repeat(cells - 1)),
        }
    }

    pub(super) fn progress_left(&self) -> String {
        "─".repeat(self.bar_width.get().saturating_sub(self.played_cells()))
    }

    pub(super) fn measure_bar(&self, event: &Event) {
        if let EventPayload::Resize { width, .. } = event.payload {
            self.bar_width.set(usize::from(width));
        }
    }

    pub(super) fn volume_label(&self) -> String {
        // Rounded: one step down from full volume is 95%, not 94%.
        let full = u32::from(u16::MAX);
        let percent = self
            .now
            .with(|now| (u32::from(now.volume) * PERCENT + full / 2) / full);
        let filled = percent.div_ceil(PERCENT / VOLUME_CELLS);
        format!(
            "{}{} {percent:>3}%",
            "▮".repeat(filled as usize),
            "▯".repeat(VOLUME_CELLS.saturating_sub(filled) as usize)
        )
    }

    pub(super) fn shuffle_on(&self) -> bool {
        self.now.with(|now| now.shuffle)
    }

    pub(super) fn repeat_mode(&self) -> Repeat {
        self.now.with(|now| now.repeat)
    }

    pub(super) fn repeat_label(&self) -> &'static str {
        match self.repeat_mode() {
            Repeat::Off | Repeat::Context => "↻ Repeat",
            Repeat::Track => "↻ Repeat one",
        }
    }

    pub(super) fn equalizer(&self) -> String {
        if !self.is_playing() {
            return EQUALIZER_LEVELS[0].to_string().repeat(EQUALIZER_BARS);
        }
        let tick = self.clock.get();
        (0..EQUALIZER_BARS)
            .map(|bar| {
                let step = (tick + bar * EQUALIZER_SPREAD) % EQUALIZER_PATTERN.len();
                EQUALIZER_LEVELS[EQUALIZER_PATTERN[step]]
            })
            .collect()
    }
}

fn label(length: Duration) -> String {
    // Saturates only past 584 million years.
    duration(length.as_millis().try_into().unwrap_or(u64::MAX))
}
