//! `beacon-icon <size> <out.png>`: the lighthouse as a `size` square app icon.
//!
//! The lighthouse lives only as `beacon-core/resources/lighthouse.svg`; the packaging scripts
//! (macOS `.icns`, the Android launcher icon) render the PNGs they need with this at build
//! time, so no raster copy of it is kept in the repository.

use std::process::ExitCode;

use resvg::{tiny_skia, usvg};

const SVG: &str = include_str!("../../beacon-core/resources/lighthouse.svg");

/// The circle's share of the square: the margin macOS and launchers expect around an icon.
const FILL: f32 = 0.912;

/// The circle's centre and radius in the SVG's own units. The art's content group is shifted
/// by (0.1, 1.79), so this is not the middle of its 192 box.
const CENTRE: (f32, f32) = (96.1, 97.79);
const RADIUS: f32 = 96.0;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(size), Some(out)) = (args.first().and_then(|s| s.parse::<u32>().ok()), args.get(1)) else {
        eprintln!("usage: beacon-icon <size> <out.png>");
        return ExitCode::FAILURE;
    };
    match render(size).and_then(|pixmap| pixmap.save_png(out).map_err(|e| e.to_string())) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("beacon-icon: {e}");
            ExitCode::FAILURE
        }
    }
}

fn render(size: u32) -> Result<tiny_skia::Pixmap, String> {
    let tree = usvg::Tree::from_str(SVG, &usvg::Options::default()).map_err(|e| e.to_string())?;
    let mut pixmap = tiny_skia::Pixmap::new(size, size).ok_or("size must be above zero")?;
    let scale = size as f32 * FILL / (2.0 * RADIUS);
    let half = size as f32 / 2.0;
    let transform = tiny_skia::Transform::from_scale(scale, scale).post_translate(half - CENTRE.0 * scale, half - CENTRE.1 * scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    Ok(pixmap)
}
