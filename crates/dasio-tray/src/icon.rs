//! The tray's icon (pure, tested everywhere): dasio's own glyph, not a
//! dot (the owner, 3.11.2026: other trays on the PC are dots already) —
//! a dark rounded square holding four level-meter bars in the state's
//! colour, so it reads as audio at 16 px and the colour still says the
//! state.

use crate::view::Colour;

/// Pixels a side.
pub const SIZE: u32 = 32;

/// The background's colour (opaque).
pub const BACKGROUND: [u8; 3] = [0x22, 0x26, 0x2e];

/// The bars' colour for a state.
pub fn bar_colour(c: Colour) -> [u8; 3] {
    match c {
        Colour::Green => [0x34, 0xd0, 0x58],
        Colour::Yellow => [0xff, 0xc1, 0x1f],
        Colour::Red => [0xff, 0x3b, 0x3b],
        Colour::Grey => [0x9a, 0x9a, 0x9a],
    }
}

/// The bars: each one's left x and height, standing on the same floor.
const BARS: [(u32, u32); 4] = [(5, 10), (11, 18), (17, 14), (23, 22)];
/// A bar's width.
const BAR_W: u32 = 4;
/// The bars' floor (the row below the last lit one).
const FLOOR: u32 = 27;
/// The square's corner radius.
const RADIUS: f32 = 6.0;

/// RGBA, rows top first, `SIZE` × `SIZE`.
pub fn rgba(c: Colour) -> Vec<u8> {
    let bar = bar_colour(c);
    let mut px = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let a = inside(x, y);
            let lit = BARS
                .iter()
                .any(|&(bx, h)| x >= bx && x < bx + BAR_W && y < FLOOR && y >= FLOOR - h);
            let [r, g, b] = if lit { bar } else { BACKGROUND };
            px.extend_from_slice(&[r, g, b, a]);
        }
    }
    px
}

/// The rounded square's coverage of pixel (x, y), 0 to 255 (soft corners).
fn inside(x: u32, y: u32) -> u8 {
    let max = (SIZE - 1) as f32;
    let (fx, fy) = (x as f32, y as f32);
    // The nearest point of the inner square (the corners' centres' box).
    let cx = fx.clamp(RADIUS, max - RADIUS);
    let cy = fy.clamp(RADIUS, max - RADIUS);
    let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
    ((RADIUS + 1.0 - d).clamp(0.0, 1.0) * 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(px: &[u8], x: u32, y: u32) -> [u8; 4] {
        let i = ((y * SIZE + x) * 4) as usize;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    }

    #[test]
    fn a_square_of_bars_not_a_dot() {
        let px = rgba(Colour::Green);
        assert_eq!(px.len(), (SIZE * SIZE * 4) as usize);
        // The square fills its edges' middles (a dot would too) and comes
        // close to its corners (a dot would not).
        assert_eq!(at(&px, 0, 16)[3], 255);
        assert_eq!(at(&px, 16, 0)[3], 255);
        assert_eq!(
            at(&px, 2, 2)[3],
            255,
            "near a corner: a square, not a circle"
        );
        assert_eq!(at(&px, 0, 0)[3], 0, "the very corner is rounded off");
        // All four corners alike.
        assert_eq!(at(&px, 31, 31)[3], 0);
        assert_eq!(at(&px, 31, 0)[3], 0);
        assert_eq!(at(&px, 0, 31)[3], 0);
        assert_eq!(at(&px, 28, 28)[3], 255);
        // The rounding is the same size at both ends.
        assert_eq!(at(&px, 1, 1)[3], 0);
        assert_eq!(at(&px, 30, 30)[3], 0);
        assert_eq!(at(&px, 31, 16)[3], 255);
        assert_eq!(at(&px, 16, 31)[3], 255);
        // The bars stand on the floor: its row is background.
        assert_eq!(at(&px, 24, 26)[..3], bar_colour(Colour::Green));
        assert_eq!(at(&px, 24, 27)[..3], BACKGROUND);
        // Bars in the state's colour, background between them.
        assert_eq!(at(&px, 24, 20), [0x34, 0xd0, 0x58, 255]);
        let [r, g, b, _] = at(&px, 9, 20);
        assert_eq!([r, g, b], BACKGROUND, "a gap between bars");
        // The tallest bar reaches higher than the shortest.
        assert_eq!(at(&px, 24, 6)[..3], bar_colour(Colour::Green));
        assert_eq!(at(&px, 6, 6)[..3], BACKGROUND);
    }

    #[test]
    fn the_state_tints_the_bars_only() {
        let (g, r) = (rgba(Colour::Green), rgba(Colour::Red));
        assert_eq!(at(&r, 24, 20)[..3], bar_colour(Colour::Red));
        assert_eq!(at(&g, 9, 20), at(&r, 9, 20), "the background stays");
        let all = [Colour::Green, Colour::Yellow, Colour::Red, Colour::Grey];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(bar_colour(*a), bar_colour(*b));
            }
        }
    }
}
