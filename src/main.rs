mod jukebox;
mod spotify;

use jukebox::{Jukebox, Startup};

const COMMAND: &str = env!("CARGO_BIN_NAME");
const HELP: &str = "\
Play Spotify in your terminal.

Usage:
  spt            open the player (logs in through your browser the first time)
  spt logout     remove your Spotify login from this computer
  spt --version  print the version

Playback needs Spotify Premium. Press ? in the player for keys.";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] => run(),
        ["logout"] => {
            spotify::forget()?;
            println!(
                "Logged out. Your Spotify login and Client ID are removed from this computer."
            );
            Ok(())
        }
        ["--version" | "-V"] => {
            println!("{COMMAND} {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        ["--help" | "-h"] => {
            println!("{HELP}");
            Ok(())
        }
        _ => Err(format!("Unknown arguments; run {COMMAND} --help for usage").into()),
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let _runtime = runtime.enter();
    let startup = Startup {
        http: reqwest::Client::new(),
        refresh_token: spotify::stored_refresh_token()?,
    };
    hypercmd::native::run(hypercmd::mount::<Jukebox>(startup)?)?;
    Ok(())
}
