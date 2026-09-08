//! Run with rustc to preview the exact production pixels, not a design mockup.
#[path = "../src/tray_visual.rs"]
mod tray_visual;

fn main() {
    let mut svg = String::from("<svg xmlns='http://www.w3.org/2000/svg' width='480' height='240' viewBox='0 0 480 240'>");
    for (row, background, foreground) in [(0, "#f8fafc", "#1e293b"), (1, "#202124", "#f1f5f9")] {
        svg.push_str(&format!("<rect y='{}' width='480' height='120' fill='{background}'/>", row * 120));
        for (column, state, label) in [
            (0, tray_visual::TrayVisual::Active, "Active"),
            (1, tray_visual::TrayVisual::Disabled, "Disabled"),
            (2, tray_visual::TrayVisual::SafetyPaused, "Safety pause"),
        ] {
            svg.push_str(&format!("<text x='{}' y='{}' fill='{foreground}' font-family='sans-serif' font-size='13'>{label}</text>", column * 160 + 12, row * 120 + 24));
            let language = ["EN", "RU", "ET"][column];
            let pixels = tray_visual::pixels(state, language);
            for (offset, scale) in [(12.0, 0.5), (48.0, 1.0), (90.0, 2.0)] {
                svg.push_str(&format!("<g transform='translate({},{}) scale({scale})'>", column as f64 * 160.0 + offset, row * 120 + 42));
                for (index, pixel) in pixels.iter().enumerate() {
                    if pixel >> 24 != 0 {
                        let alpha = pixel >> 24;
                        let red = ((pixel >> 16) & 255) * 255 / alpha;
                        let green = ((pixel >> 8) & 255) * 255 / alpha;
                        let blue = (pixel & 255) * 255 / alpha;
                        svg.push_str(&format!("<rect x='{}' y='{}' width='1' height='1' fill='rgb({red},{green},{blue})' fill-opacity='{}'/>", index % 32, index / 32, alpha as f32 / 255.0));
                    }
                }
                svg.push_str("</g>");
            }
        }
    }
    svg.push_str("</svg>");
    std::fs::write(std::env::args().nth(1).expect("output SVG path"), svg).unwrap();
}
