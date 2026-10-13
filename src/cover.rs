//! Album covers in the now-playing bar, through the kitty graphics protocol.
//!
//! The cover is sent to the terminal once per track, then shown with Unicode
//! placeholders: ordinary text cells whose foreground color names the image and
//! whose diacritics give the row and column. The interface paints them like any
//! other text, and tmux redraws them on its own, so no cursor positioning is needed.
use image::imageops::FilterType;
use std::io::{self, Write};

/// Cells the cover takes in the now-playing bar.
pub(crate) const COLUMNS: usize = 6;
pub(crate) const ROWS: usize = 3;
/// Side of the image sent to the terminal; the terminal scales it to the cells.
const PIXELS: u32 = 160;
/// Spotify sends 64, 300 and 640 pixel covers; the smallest that is sharp enough wins.
const SHARP_ENOUGH: i32 = 160;
const CHUNK: usize = 4096;
/// The radius of the rounded corners, as a share of the cover's side.
const CORNER: f32 = 0.12;
const PLACEHOLDER: char = '\u{10EEEE}';
/// The first row and column diacritics of kitty's `rowcolumn-diacritics.txt`.
const DIACRITICS: [char; COLUMNS] = [
    '\u{0305}', '\u{030D}', '\u{030E}', '\u{0310}', '\u{0312}', '\u{033D}',
];
/// The color of the placeholders, which is also the image's ID: #5F87AF in true
/// color, or the exact 256-color cube entry 67 that hypercmd turns it into otherwise.
const TRUE_COLOR_ID: u32 = 0x5F_87AF;
const INDEXED_ID: u32 = 67;

/// Whether this terminal shows images; inside tmux, kitty's variables are inherited.
/// The placeholders also need a color, which hypercmd leaves out without one of these.
pub(crate) fn supported() -> bool {
    let kitty = std::env::var_os("KITTY_WINDOW_ID").is_some()
        || std::env::var("TERM").is_ok_and(|term| term == "xterm-kitty");
    let colored = std::env::var_os("NO_COLOR").is_none()
        && (true_color() || std::env::var("TERM").is_ok_and(|term| term.contains("256color")));
    kitty && colored
}

/// The text of one row of the cover.
pub(crate) fn row(row: usize) -> String {
    DIACRITICS
        .iter()
        .take(COLUMNS)
        .flat_map(|column| [PLACEHOLDER, DIACRITICS[row], *column])
        .collect()
}

/// The URL of the cover to fetch among the sizes Spotify offers.
pub(crate) fn pick<'a>(sizes: impl Iterator<Item = (i32, &'a str)>) -> Option<String> {
    let mut sizes: Vec<_> = sizes.collect();
    sizes.sort_by_key(|(width, _)| *width);
    sizes
        .iter()
        .find(|(width, _)| *width >= SHARP_ENOUGH)
        .or(sizes.last())
        .map(|(_, url)| (*url).to_owned())
}

/// Downloads a cover and shrinks it to raw RGBA pixels, corners rounded.
pub(crate) async fn fetch(
    http: reqwest::Client,
    url: String,
) -> Result<Vec<u8>, crate::spotify::Error> {
    let bytes = tokio::spawn(async move {
        http.get(url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await
    })
    .await??;
    let picture = image::load_from_memory(&bytes)
        .map_err(|error| crate::spotify::Error::Cover(error.to_string()))?;
    let mut pixels = picture
        .resize_exact(PIXELS, PIXELS, FilterType::Triangle)
        .to_rgba8();
    round_corners(&mut pixels);
    Ok(pixels.into_raw())
}

/// Makes the corners transparent, with a smooth edge one pixel wide.
fn round_corners(pixels: &mut image::RgbaImage) {
    let side = pixels.width() as f32;
    let radius = side * CORNER;
    for (x, y, pixel) in pixels.enumerate_pixels_mut() {
        let coverage = corner_coverage(x as f32 + 0.5, y as f32 + 0.5, side, radius);
        pixel[3] = (f32::from(pixel[3]) * coverage).round() as u8;
    }
}

/// How much of the pixel centered on (`x`, `y`) lies inside the rounded square.
fn corner_coverage(x: f32, y: f32, side: f32, radius: f32) -> f32 {
    // The distance past the nearest corner's circle center, along each axis.
    let outside_x = (radius - x).max(x - (side - radius)).max(0.0);
    let outside_y = (radius - y).max(y - (side - radius)).max(0.0);
    if outside_x == 0.0 || outside_y == 0.0 {
        return 1.0;
    }
    (radius - outside_x.hypot(outside_y) + 0.5).clamp(0.0, 1.0)
}

/// Replaces the cover the terminal holds with `pixels`.
pub(crate) fn show(pixels: &[u8]) -> io::Result<()> {
    let id = image_id();
    let payload = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, pixels);
    let mut commands = vec![delete()];
    let chunks: Vec<_> = payload.as_bytes().chunks(CHUNK).collect();
    for (index, chunk) in chunks.iter().enumerate() {
        let more = u8::from(index + 1 < chunks.len());
        let keys = if index == 0 {
            format!("a=T,U=1,i={id},f=32,s={PIXELS},v={PIXELS},c={COLUMNS},r={ROWS},q=2,m={more}")
        } else {
            format!("q=2,m={more}")
        };
        commands.push(format!(
            "\x1b_G{keys};{}\x1b\\",
            String::from_utf8_lossy(chunk)
        ));
    }
    let mut output = io::stdout().lock();
    for command in commands {
        output.write_all(through_tmux(command).as_bytes())?;
    }
    output.flush()
}

/// Frees the cover the terminal holds, when a track has none or the player exits.
pub(crate) fn clear() -> io::Result<()> {
    let mut output = io::stdout().lock();
    output.write_all(through_tmux(delete()).as_bytes())?;
    output.flush()
}

fn delete() -> String {
    format!("\x1b_Ga=d,d=I,i={},q=2\x1b\\", image_id())
}

/// The same test hypercmd uses before downgrading colors to 256.
fn true_color() -> bool {
    std::env::var("COLORTERM").is_ok_and(|color| matches!(color.as_str(), "truecolor" | "24bit"))
}

fn image_id() -> u32 {
    if true_color() {
        TRUE_COLOR_ID
    } else {
        INDEXED_ID
    }
}

/// tmux forwards an escape sequence to the outer terminal only when wrapped.
fn through_tmux(command: String) -> String {
    if std::env::var_os("TMUX").is_none() {
        return command;
    }
    format!("\x1bPtmux;{}\x1b\\", command.replace('\x1b', "\x1b\x1b"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn corners_are_cut_round() {
        let (side, radius) = (100.0, 12.0);
        let coverage = |x, y| super::corner_coverage(x, y, side, radius);
        assert_eq!(coverage(0.5, 0.5), 0.0);
        assert_eq!(coverage(99.5, 99.5), 0.0);
        assert_eq!(coverage(50.0, 0.5), 1.0);
        assert_eq!(coverage(0.5, 50.0), 1.0);
        assert_eq!(coverage(12.0, 12.0), 1.0);
        let on_circle = 12.0 - 12.0 * std::f32::consts::FRAC_1_SQRT_2;
        let edge = coverage(on_circle, on_circle);
        assert!(edge > 0.3 && edge < 0.7, "{edge}");
    }
}
