use super::{Jukebox, Screen, listing::Source};
use crate::spotify::{Command, Repeat};
use std::rc::Rc;

const LOGO: &str = include_str!("../../assets/logo.txt");

macro_rules! jukebox_views {
    ($($name:ident => $template:literal),+ $(,)?) => {
        $(
            #[derive(fusor::FromInputs)]
            struct $name {
                #[input]
                jukebox: Rc<Jukebox>,
            }

            fusor::template!(backend = "hypercmd", $template);
        )+
    };
}

jukebox_views! {
    Welcome => "ui/welcome.html",
    Shelves => "ui/shelves.html",
    TrackList => "ui/tracks.html",
    PlayerBar => "ui/player-bar.html",
    Help => "ui/help.html",
}

fusor::template!(backend = "hypercmd", "ui/app.html");
