# Play Spotify. Stay in your terminal.

spotify-tui is a Spotify player for macOS and Linux. Browse your Liked Songs,
playlists and recent plays, search the catalog, and jump to a track's album or
artist. Music plays directly on your computer. The installed executable is
named `spt`; the Cargo package is `fusor-spotify-tui`.

The terminal interface is written in HTML and CSS with
[Hypercmd](https://github.com/fusor-rs/hypercmd) and
[Fusor](https://github.com/fusor-rs/fusor). Login, browsing and audio use
[librespot](https://github.com/librespot-org/librespot).

**In development · v0.1** — See [status and known limitations](#status-and-known-limitations).

[Get started](#get-started) · [Using spotify-tui](#using-spotify-tui) ·
[How it works](#how-it-works) · [Status](#status-and-known-limitations) ·
[Contributing](#contributing)

## Get started

You need a Spotify Premium account, a terminal on macOS or Linux, and a browser
for the first login. No Spotify developer app or client ID setup is required.

On Linux, audio uses ALSA and login storage requires an unlocked Secret Service
provider, such as GNOME Keyring, on your session D-Bus. Install `xdg-utils` to
let the app open the login page in your browser.

### Install a binary release

The shell installer downloads the latest published GitHub release for x86-64 or
ARM64 on macOS or Linux. It verifies the SHA-256 checksum and executable version,
then installs `spt` in `~/.spotify-tui/bin` without sudo or a Rust toolchain:

```sh
curl -fsSL https://raw.githubusercontent.com/fusor-rs/spotify-tui/main/install.sh | sh
export PATH="$HOME/.spotify-tui/bin:$PATH"
spt
```

Add the `export` line to your shell profile, such as `~/.zshrc` or `~/.bashrc`.
Run the installer again to upgrade. To choose a release, append `-s -- v0.1.0`
to `sh`, or set `SPOTIFY_TUI_VERSION` on the `sh` command. Set
`SPOTIFY_TUI_INSTALL` on that command to choose another install directory; the
executable goes in its `bin/`.

Linux binaries target glibc 2.35 or newer and need the ALSA and OpenSSL 3 runtime
libraries, as provided by Ubuntu 22.04 or newer. Alpine's musl runtime is not
supported by these archives. The installer requires `curl`, `tar`, and either
`sha256sum` or `shasum`.

This method requires a [GitHub release](https://github.com/fusor-rs/spotify-tui/releases)
with completed binary uploads.

### Install with Cargo

Install the current stable Rust toolchain through [rustup](https://rustup.rs).
On Linux, install the build dependencies first; on Debian or Ubuntu:

```sh
sudo apt-get update
sudo apt-get install build-essential pkg-config libasound2-dev libssl-dev
```

On macOS, building requires the Xcode Command Line Tools (`xcode-select --install`).
After the first release is published to crates.io, install with:

```sh
cargo install fusor-spotify-tui --locked
spt
```

`--locked` uses the release's recorded dependency versions. Cargo installs `spt`
in `~/.cargo/bin` by default; that directory needs to be in your PATH. Run the
same command again to upgrade. Check your installed version with `spt --version`.

### Log in and listen

1. Run `spt` and choose **Log in with Spotify**.
2. Approve access in the browser. The terminal continues automatically after
   the browser returns to `http://127.0.0.1:8898/login`.
3. Choose a library section or playlist, select a track, and press Enter.

Your login is stored in macOS Keychain or Linux Secret Service, so later starts
open your library. Run `spt logout` to remove the saved login. Upgrading the
executable preserves it; macOS can ask you to allow the updated executable to
read its keychain item.

### Build from source

With Rust and the platform build dependencies installed:

```sh
git clone https://github.com/fusor-rs/spotify-tui.git
cd spotify-tui
cargo install --path . --locked
spt
```

For development, `cargo run --locked` builds and launches the app from the checkout.

## Using spotify-tui

### Browse your library

The sidebar lists your library and playlists. Choose a section to see its tracks,
then use the arrow keys and Enter to start playback. Press `/` or Ctrl+F to search
the catalog. With a track selected, press `o` to open its album or `a` to open
its artist; Escape goes back.

### Control playback

The bar at the bottom shows what's playing. Use its buttons or the keyboard to
pause, skip, seek, change volume, shuffle, or cycle repeat modes. While `spt`
runs, other Spotify apps list this computer as **spotify-tui**, so you can also
control its playback from your phone.

### Keyboard and mouse

Press `?` in the player to see the controls. Click the sidebar and playback
buttons, or scroll the track list with the mouse wheel.

<details>
<summary>Controls</summary>

| Key | Action |
| --- | --- |
| `Space` | Play or pause |
| `n` / `p` | Next / previous track |
| `←` / `→` | Seek 10 seconds |
| `+` / `-` | Volume |
| `s` / `r` | Shuffle / cycle repeat |
| `↑` / `↓` / `PgUp` / `PgDn` | Choose a row |
| `Enter` | Play the selected track |
| `o` / `a` | Open the selected track's album / artist |
| `Esc` / `Backspace` | Go back |
| `/` / `Ctrl+F` | Search |
| `Tab` / `Shift+Tab` | Move between controls and panes |
| `?` | Show controls |
| `q` / `Ctrl+C` | Quit |

Single-key shortcuts apply outside text inputs.

</details>

## How it works

- **Browser login.** Spotify's OAuth authorization-code flow uses PKCE and a
  local callback on `127.0.0.1:8898`. The refresh token lives in the system
  credential store.
- **Library and catalog.** Playlists, contexts, search and track metadata use
  Spotify's internal services through a librespot session. The app does not use
  the public Spotify Web API.
- **Local playback.** An in-process Spotify Connect speaker streams audio through
  Rodio. Player events update the now-playing bar.
- **HTML views, Rust state.** Hypercmd compiles the [HTML templates](ui/) at build
  time and connects them to [Rust state](src/jukebox/). Templates, styles and
  embedded assets are included in the crate package.

## Status and known limitations

spotify-tui is in early development and requires Spotify Premium for playback.

| Capability | Status |
| --- | --- |
| Library | Liked Songs, playlists and recent plays |
| Discovery | Catalog search and album / artist navigation |
| Playback | Local audio, seeking, volume, shuffle and repeat |
| Spotify Connect | Control this player's playback from other Spotify apps |
| Login storage | macOS Keychain or Linux Secret Service |
| Terminal platforms | macOS and Linux |

Queue editing, liking tracks, podcasts and controlling other devices from the
terminal are not implemented. Windows is not supported. Spotify's internal
services are undocumented and can change independently of this app.

## Contributing

Views and styles live in `ui/`, application state in `src/jukebox/`, and login,
catalog and playback integration in `src/spotify/`. Dependency versions are pinned.

Run the checks before submitting a change:

```sh
sh -n install.sh
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo publish --locked --dry-run
```

The [Check workflow](.github/workflows/check.yml) runs on Linux and macOS for
pushes and pull requests. It checks formatting and lints, runs tests, verifies
the packaged crate, and launches the executable's help and version commands.

### Releases

Create a GitHub environment named `crates-io` with a `CARGO_REGISTRY_TOKEN` secret
authorized to publish `fusor-spotify-tui`. Commit the release version in
`Cargo.toml` and `Cargo.lock`, then publish a GitHub release with the matching
`vVERSION` tag, such as `v0.1.0`.

The [Release workflow](.github/workflows/release.yml) runs Check, validates the
tag against the manifest, and builds x86-64 and ARM64 executables on Linux and
macOS. Linux builds use Ubuntu 22.04 with GNU targets to link the audio and TLS
libraries. Each archive contains `spt`, `LICENSE`, and `README.md` in a directory
named `spotify-tui-VERSION-TARGET`. Archives have a `.tar.gz` extension and an
accompanying `.tar.gz.sha256` checksum file.

Each build uses [install.sh](install.sh) to verify, install and run its executable
before uploading artifacts. It also checks that a corrupt download leaves the
installed executable unchanged. `SPOTIFY_TUI_DOWNLOAD_BASE` points the installer
at local archives during these checks.

After all builds pass, the workflow attaches the archives to the GitHub release
and publishes the crate to crates.io. Reruns skip an already published crate
version and replace release assets. The installer is attached after both steps
succeed. Running Release manually verifies the crate and produces binary workflow
artifacts without publishing a crate or uploading GitHub release assets.

## License

Licensed under the [MIT License](LICENSE).
