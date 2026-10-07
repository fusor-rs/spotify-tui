use super::{Jukebox, Screen, playback::VolumeStep};
use crate::spotify::Command;
use hypercmd::{Error, ErrorKind, Event, EventPayload, Input, Key};
use std::rc::Rc;

impl Jukebox {
    pub(super) fn shortcut(self: &Rc<Self>, event: &Event) -> Result<(), Error> {
        let EventPayload::Input(Input::Key { key, modifiers, .. }) = event.payload else {
            return Ok(());
        };
        if self.screen.get_untracked() != Screen::Library {
            return Ok(());
        }
        if self.help.get_untracked() {
            if key == Key::Escape {
                self.close_help();
                event.prevent_default();
            }
            return Ok(());
        }
        if modifiers.control || modifiers.alt || modifiers.super_key {
            if key == Key::Char('f') && modifiers.control {
                self.start_search();
                event.prevent_default();
            }
            return Ok(());
        }
        if event.target.tag() == "input" || !self.single_key(key)? {
            return Ok(());
        }
        event.prevent_default();
        Ok(())
    }

    /// Handles a single-key shortcut; returns whether `key` was one.
    fn single_key(self: &Rc<Self>, key: Key) -> Result<bool, Error> {
        match key {
            Key::Char(' ') => self.toggle_play()?,
            Key::Char('n') => self.command(Command::Next),
            Key::Char('p') => self.command(Command::Previous),
            Key::Right => self.seek_forward(),
            Key::Left => self.seek_back(),
            Key::Char('+' | '=') => self.change_volume(VolumeStep::Up),
            Key::Char('-') => self.change_volume(VolumeStep::Down),
            Key::Char('s') => self.toggle_shuffle(),
            Key::Char('r') => self.cycle_repeat(),
            Key::Char('o') => self.open_album(),
            Key::Char('a') => self.open_artist(),
            Key::Char('/') => self.start_search(),
            Key::Char('?') => self.open_help(),
            Key::Char('q') => quit()?,
            Key::Escape | Key::Backspace => return Ok(self.back()),
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Tab leaves the sidebar for the track list instead of visiting every playlist.
    pub(super) fn leave_shelves(&self, event: &Event) {
        if let EventPayload::Input(Input::Key {
            key: Key::Tab,
            modifiers,
            ..
        }) = event.payload
            && !modifiers.shift
        {
            self.focus("tracks");
            event.prevent_default();
        }
    }

    pub(super) fn open_help(&self) {
        self.help.set(true);
    }

    pub(super) fn close_help(&self) {
        self.help.set(false);
        self.focus("tracks");
    }
}

/// Asks the terminal runner for the same orderly shutdown as Ctrl+C.
fn quit() -> Result<(), Error> {
    signal_hook::low_level::raise(signal_hook::consts::SIGTERM)
        .map_err(|error| Error::new(ErrorKind::Terminal, format!("cannot quit: {error}")))
}
