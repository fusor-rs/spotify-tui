//! Album covers in the now-playing bar, through the kitty graphics protocol.
//!
//! The cover is sent to the terminal once per track, then shown with Unicode
//! placeholders: ordinary text cells whose foreground color and diacritics name
//! the image, the row and the column. The interface paints them like any
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
const PLACEHOLDER: char = '\u{10EEEE}';
/// The diacritics of kitty's `rowcolumn-diacritics.txt` that stand for 0 to 255.
const DIACRITICS: [char; 256] = [
    '\u{0305}', '\u{030D}', '\u{030E}', '\u{0310}', '\u{0312}', '\u{033D}', '\u{033E}', '\u{033F}',
    '\u{0346}', '\u{034A}', '\u{034B}', '\u{034C}', '\u{0350}', '\u{0351}', '\u{0352}', '\u{0357}',
    '\u{035B}', '\u{0363}', '\u{0364}', '\u{0365}', '\u{0366}', '\u{0367}', '\u{0368}', '\u{0369}',
    '\u{036A}', '\u{036B}', '\u{036C}', '\u{036D}', '\u{036E}', '\u{036F}', '\u{0483}', '\u{0484}',
    '\u{0485}', '\u{0486}', '\u{0487}', '\u{0592}', '\u{0593}', '\u{0594}', '\u{0595}', '\u{0597}',
    '\u{0598}', '\u{0599}', '\u{059C}', '\u{059D}', '\u{059E}', '\u{059F}', '\u{05A0}', '\u{05A1}',
    '\u{05A8}', '\u{05A9}', '\u{05AB}', '\u{05AC}', '\u{05AF}', '\u{05C4}', '\u{0610}', '\u{0611}',
    '\u{0612}', '\u{0613}', '\u{0614}', '\u{0615}', '\u{0616}', '\u{0617}', '\u{0657}', '\u{0658}',
    '\u{0659}', '\u{065A}', '\u{065B}', '\u{065D}', '\u{065E}', '\u{06D6}', '\u{06D7}', '\u{06D8}',
    '\u{06D9}', '\u{06DA}', '\u{06DB}', '\u{06DC}', '\u{06DF}', '\u{06E0}', '\u{06E1}', '\u{06E2}',
    '\u{06E4}', '\u{06E7}', '\u{06E8}', '\u{06EB}', '\u{06EC}', '\u{0730}', '\u{0732}', '\u{0733}',
    '\u{0735}', '\u{0736}', '\u{073A}', '\u{073D}', '\u{073F}', '\u{0740}', '\u{0741}', '\u{0743}',
    '\u{0745}', '\u{0747}', '\u{0749}', '\u{074A}', '\u{07EB}', '\u{07EC}', '\u{07ED}', '\u{07EE}',
    '\u{07EF}', '\u{07F0}', '\u{07F1}', '\u{07F3}', '\u{0816}', '\u{0817}', '\u{0818}', '\u{0819}',
    '\u{081B}', '\u{081C}', '\u{081D}', '\u{081E}', '\u{081F}', '\u{0820}', '\u{0821}', '\u{0822}',
    '\u{0823}', '\u{0825}', '\u{0826}', '\u{0827}', '\u{0829}', '\u{082A}', '\u{082B}', '\u{082C}',
    '\u{082D}', '\u{0951}', '\u{0953}', '\u{0954}', '\u{0F82}', '\u{0F83}', '\u{0F86}', '\u{0F87}',
    '\u{135D}', '\u{135E}', '\u{135F}', '\u{17DD}', '\u{193A}', '\u{1A17}', '\u{1A75}', '\u{1A76}',
    '\u{1A77}', '\u{1A78}', '\u{1A79}', '\u{1A7A}', '\u{1A7B}', '\u{1A7C}', '\u{1B6B}', '\u{1B6D}',
    '\u{1B6E}', '\u{1B6F}', '\u{1B70}', '\u{1B71}', '\u{1B72}', '\u{1B73}', '\u{1CD0}', '\u{1CD1}',
    '\u{1CD2}', '\u{1CDA}', '\u{1CDB}', '\u{1CE0}', '\u{1DC0}', '\u{1DC1}', '\u{1DC3}', '\u{1DC4}',
    '\u{1DC5}', '\u{1DC6}', '\u{1DC7}', '\u{1DC8}', '\u{1DC9}', '\u{1DCB}', '\u{1DCC}', '\u{1DD1}',
    '\u{1DD2}', '\u{1DD3}', '\u{1DD4}', '\u{1DD5}', '\u{1DD6}', '\u{1DD7}', '\u{1DD8}', '\u{1DD9}',
    '\u{1DDA}', '\u{1DDB}', '\u{1DDC}', '\u{1DDD}', '\u{1DDE}', '\u{1DDF}', '\u{1DE0}', '\u{1DE1}',
    '\u{1DE2}', '\u{1DE3}', '\u{1DE4}', '\u{1DE5}', '\u{1DE6}', '\u{1DFE}', '\u{20D0}', '\u{20D1}',
    '\u{20D4}', '\u{20D5}', '\u{20D6}', '\u{20D7}', '\u{20DB}', '\u{20DC}', '\u{20E1}', '\u{20E7}',
    '\u{20E9}', '\u{20F0}', '\u{2CEF}', '\u{2CF0}', '\u{2CF1}', '\u{2DE0}', '\u{2DE1}', '\u{2DE2}',
    '\u{2DE3}', '\u{2DE4}', '\u{2DE5}', '\u{2DE6}', '\u{2DE7}', '\u{2DE8}', '\u{2DE9}', '\u{2DEA}',
    '\u{2DEB}', '\u{2DEC}', '\u{2DED}', '\u{2DEE}', '\u{2DEF}', '\u{2DF0}', '\u{2DF1}', '\u{2DF2}',
    '\u{2DF3}', '\u{2DF4}', '\u{2DF5}', '\u{2DF6}', '\u{2DF7}', '\u{2DF8}', '\u{2DF9}', '\u{2DFA}',
    '\u{2DFB}', '\u{2DFC}', '\u{2DFD}', '\u{2DFE}', '\u{2DFF}', '\u{A66F}', '\u{A67C}', '\u{A67D}',
    '\u{A6F0}', '\u{A6F1}', '\u{A8E0}', '\u{A8E1}', '\u{A8E2}', '\u{A8E3}', '\u{A8E4}', '\u{A8E5}',
];
/// The color of the placeholders, which is also the low bits of the image's ID:
/// #5F87AF in true color, or the exact 256-color cube entry 67 that hypercmd turns
/// it into otherwise. The high byte comes from [`instance`].
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
    placeholders(row, instance())
}

/// Each cell names its row, its column, then the high byte of the image's ID.
fn placeholders(row: usize, instance: u8) -> String {
    DIACRITICS
        .iter()
        .take(COLUMNS)
        .flat_map(|column| {
            [
                PLACEHOLDER,
                DIACRITICS[row],
                *column,
                DIACRITICS[usize::from(instance)],
            ]
        })
        .collect()
}

/// The high byte of the image's ID, never 0 and different for each process, so
/// two players in one kitty window do not replace each other's cover.
fn instance() -> u8 {
    // Between 1 and 255, so the conversion cannot fail.
    u8::try_from(std::process::id() % 255 + 1).unwrap_or(u8::MAX)
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

/// Downloads a cover and shrinks it to raw RGB pixels.
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
    Ok(picture
        .resize_exact(PIXELS, PIXELS, FilterType::Triangle)
        .to_rgb8()
        .into_raw())
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
            format!("a=T,U=1,i={id},f=24,s={PIXELS},v={PIXELS},c={COLUMNS},r={ROWS},q=2,m={more}")
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
    id(instance(), true_color())
}

fn id(instance: u8, true_color: bool) -> u32 {
    let color = if true_color {
        TRUE_COLOR_ID
    } else {
        INDEXED_ID
    };
    u32::from(instance) << 24 | color
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
    use super::{COLUMNS, DIACRITICS, PLACEHOLDER, id, placeholders};

    #[test]
    fn placeholders_carry_row_column_and_instance() {
        let row: Vec<char> = placeholders(2, 200).chars().collect();
        assert_eq!(row.len(), COLUMNS * 4);
        assert_eq!(
            row.get(4..8),
            Some(&[PLACEHOLDER, '\u{030E}', '\u{030D}', DIACRITICS[200]][..])
        );
        // The last entry kitty needs for a byte, from rowcolumn-diacritics.txt.
        assert_eq!(DIACRITICS[255], '\u{A8E5}');
    }

    #[test]
    fn ids_keep_the_color_in_the_low_bits() {
        assert_eq!(id(200, true), 0xC85F_87AF);
        assert_eq!(id(1, false), 0x0100_0043);
    }
}
