//! Compact layout badge. Pixels are opaque ARGB for Win32.
pub const SIZE: usize = 32;
pub const ACTIVE_BACKGROUND: u32 = 0xFF25_63EB;
pub const DISABLED_BACKGROUND: u32 = 0xFF6B_7280;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayVisual {
    Active,
    Disabled,
    SafetyPaused,
}

// Unit-coordinate strokes for the supported two-letter layout labels.
fn strokes(letter: char) -> &'static [(f32, f32, f32, f32)] {
    match letter {
        'E' => &[
            (0., 0., 0., 1.),
            (0., 0., 1., 0.),
            (0., 0.5, 0.8, 0.5),
            (0., 1., 1., 1.),
        ],
        'N' => &[(0., 1., 0., 0.), (0., 0., 1., 1.), (1., 1., 1., 0.)],
        'R' => &[
            (0., 1., 0., 0.),
            (0., 0., 0.75, 0.),
            (0.75, 0., 1., 0.15),
            (1., 0.15, 1., 0.35),
            (1., 0.35, 0.75, 0.5),
            (0.75, 0.5, 0., 0.5),
            (0.4, 0.5, 1., 1.),
        ],
        'U' => &[
            (0., 0., 0., 0.8),
            (0., 0.8, 0.2, 1.),
            (0.2, 1., 0.8, 1.),
            (0.8, 1., 1., 0.8),
            (1., 0.8, 1., 0.),
        ],
        'T' => &[(0., 0., 1., 0.), (0.5, 0., 0.5, 1.)],
        'J' => &[
            (0., 0., 1., 0.),
            (1., 0., 1., 0.8),
            (1., 0.8, 0.75, 1.),
            (0.75, 1., 0.25, 1.),
            (0.25, 1., 0., 0.8),
        ],
        'A' => &[(0., 1., 0.5, 0.), (0.5, 0., 1., 1.), (0.2, 0.6, 0.8, 0.6)],
        _ => &[
            (0., 0.15, 0.25, 0.),
            (0.25, 0., 0.75, 0.),
            (0.75, 0., 1., 0.15),
            (1., 0.15, 1., 0.35),
            (1., 0.35, 0.5, 0.55),
            (0.5, 0.55, 0.5, 0.65),
            (0.5, 0.9, 0.5, 1.),
        ],
    }
}

pub fn pixels(state: TrayVisual, label: &str) -> Vec<u32> {
    let background = if state == TrayVisual::Active {
        ACTIVE_BACKGROUND
    } else {
        DISABLED_BACKGROUND
    };
    let label = if label.chars().count() == 2 {
        label
    } else {
        "??"
    };
    let lines: Vec<_> = label
        .chars()
        .enumerate()
        .flat_map(|(index, letter)| {
            strokes(letter).iter().map(move |&(ax, ay, bx, by)| {
                let origin = 5.0 + index as f32 * 14.0;
                (
                    origin + ax * 8.0,
                    8.0 + ay * 16.0,
                    origin + bx * 8.0,
                    8.0 + by * 16.0,
                )
            })
        })
        .collect();
    let mut result = vec![background; SIZE * SIZE];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let mut coverage = 0u32;
            for sy in 0..4 {
                for sx in 0..4 {
                    let px = x as f32 + (sx as f32 + 0.5) / 4.0;
                    let py = y as f32 + (sy as f32 + 0.5) / 4.0;
                    if lines.iter().any(|&(ax, ay, bx, by)| {
                        let dx = bx - ax;
                        let dy = by - ay;
                        let t = (((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy))
                            .clamp(0.0, 1.0);
                        (px - ax - t * dx).hypot(py - ay - t * dy) <= 1.1
                    }) {
                        coverage += 1;
                    }
                }
            }
            let mut pixel = 0xFF00_0000;
            for shift in [16, 8, 0] {
                let base = (background >> shift) & 255;
                pixel |= (base + (255 - base) * coverage / 16) << shift;
            }
            result[y * SIZE + x] = pixel;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_badge_uses_mode_color_without_a_frame_or_warning_dot() {
        for (state, background) in [
            (TrayVisual::Active, ACTIVE_BACKGROUND),
            (TrayVisual::Disabled, DISABLED_BACKGROUND),
            (TrayVisual::SafetyPaused, DISABLED_BACKGROUND),
        ] {
            let image = pixels(state, "EN");
            assert!(image.contains(&0xFFFF_FFFF));
            assert!(image.iter().all(|pixel| pixel >> 24 == 255));
            for n in 0..SIZE {
                assert_eq!(image[n], background);
                assert_eq!(image[(SIZE - 1) * SIZE + n], background);
                assert_eq!(image[n * SIZE], background);
                assert_eq!(image[n * SIZE + SIZE - 1], background);
            }
        }
        assert_eq!(
            pixels(TrayVisual::Disabled, "RU"),
            pixels(TrayVisual::SafetyPaused, "RU")
        );
    }

    #[test]
    fn every_layout_has_a_distinct_white_label_in_both_modes() {
        let labels = ["EN", "RU", "ET", "JA", "??"];
        for state in [TrayVisual::Active, TrayVisual::Disabled] {
            for (index, label) in labels.iter().enumerate() {
                let image = pixels(state, label);
                assert!(image.contains(&0xFFFF_FFFF));
                for previous in &labels[..index] {
                    assert_ne!(image, pixels(state, previous));
                }
            }
        }
    }
}
