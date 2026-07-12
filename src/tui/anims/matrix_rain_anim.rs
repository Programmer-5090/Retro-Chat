use ratatui::{
    layout::Rect,
    style::{ Color, Style },
    text::{ Line, Span },
    widgets::Paragraph,
    Frame,
};

/// Half-width katakana render as a single terminal cell in most monospace
/// fonts (unlike full-width katakana). Mixed in with
/// digits for a bit of variety.
const CHARSET: &[char] = &[
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9',
    'ｱ', 'ｲ', 'ｳ', 'ｴ', 'ｵ', 'ｶ', 'ｷ', 'ｸ', 'ｹ', 'ｺ',
    'ｻ', 'ｼ', 'ｽ', 'ｾ', 'ｿ', 'ﾀ', 'ﾁ', 'ﾂ', 'ﾃ', 'ﾄ',
    'ﾅ', 'ﾆ', 'ﾇ', 'ﾈ', 'ﾉ', 'ﾊ', 'ﾋ', 'ﾌ', 'ﾍ', 'ﾎ',
];

fn hash_u32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846ca68b);
    x ^= x >> 16;
    x
}

pub struct MatrixRain {
    pub color: Color,
}

impl MatrixRain {
    pub fn new() -> Self {
        Self {
            color: Color::Rgb(0, 255, 70),
        }
    }

    fn dim(&self, brightness: f64) -> Color {
        let (r, g, b) = match self.color {
            Color::Rgb(r, g, b) => (r, g, b),
            _ => (0, 255, 70),
        };
        let f = 0.15 + brightness.clamp(0.0, 1.0) * 0.85;
        Color::Rgb(((r as f64) * f) as u8, ((g as f64) * f) as u8, ((b as f64) * f) as u8)
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, t: f64) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        // Per-column drop parameters, derived once per frame from a hash of
        // the column index so every column falls at its own speed/phase.
        let cols: Vec<(f64, f64, i32)> = (0..area.width)
            .map(|col| {
                let seed = hash_u32(col as u32);
                let speed = 4.0 + ((seed % 700) as f64) / 100.0; // 4.0..11.0 rows/sec
                let phase = ((hash_u32(seed) % 1000) as f64) / 1000.0;
                let drop_len = 6 + ((hash_u32(seed.wrapping_add(1)) % 10) as i32); // 6..16
                (speed, phase, drop_len)
            })
            .collect();

        let mut lines: Vec<Line> = Vec::with_capacity(area.height as usize);
        for row in 0..area.height {
            let mut spans: Vec<Span> = Vec::with_capacity(area.width as usize);
            for col in 0..area.width {
                let (speed, phase, drop_len) = cols[col as usize];
                let total = (area.height as f64) + (drop_len as f64);
                let head = (t * speed + phase * total).rem_euclid(total) - (drop_len as f64);
                let dist = head - (row as f64);

                if dist < 0.0 || dist >= (drop_len as f64) {
                    spans.push(Span::raw(" "));
                    continue;
                }

                let brightness = 1.0 - dist / (drop_len as f64);
                let time_bucket = (t * 8.0) as u32;
                let glyph_seed = hash_u32(
                    (col as u32).wrapping_mul(7919) ^
                        (row as u32).wrapping_mul(104729) ^
                        time_bucket
                );
                let ch = CHARSET[(glyph_seed as usize) % CHARSET.len()];

                let color = if dist < 1.0 {
                    // Leading glyph of each drop reads bright, near-white.
                    Color::Rgb(210, 255, 210)
                } else {
                    self.dim(brightness)
                };

                spans.push(Span::styled(ch.to_string(), Style::default().fg(color)));
            }
            lines.push(Line::from(spans));
        }

        let p = Paragraph::new(lines);
        frame.render_widget(p, area);
    }
}
