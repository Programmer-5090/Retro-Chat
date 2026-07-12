use ratatui::{
    layout::Rect,
    style::Color,
    symbols::Marker,
    widgets::canvas::{ Canvas, Points },
    Frame,
};

fn hash_u32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846ca68b);
    x ^= x >> 16;
    x
}

pub struct Starfield {
    pub color: Color,
    pub count: usize,
}

impl Starfield {
    pub fn new() -> Self {
        Self {
            color: Color::Rgb(255, 255, 255),
            count: 50,
        }
    }

    fn dim(&self, brightness: f64) -> Color {
        let (r, g, b) = match self.color {
            Color::Rgb(r, g, b) => (r, g, b),
            _ => (255, 255, 255),
        };
        let f = 0.2 + brightness.clamp(0.0, 1.0) * 0.8;
        Color::Rgb(((r as f64) * f) as u8, ((g as f64) * f) as u8, ((b as f64) * f) as u8)
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, t: f64) {
        let w = (area.width.max(1)) as f64;
        let h = (area.height.max(1)) as f64;
        let bound_x = 20.0;
        let bound_y = bound_x * (2.0 * h) / w; // compensate for cell dot ratio

        let count = self.count;
        let canvas = Canvas::default()
            .marker(Marker::Braille)
            .x_bounds([-bound_x, bound_x])
            .y_bounds([-bound_y, bound_y])
            .paint(|ctx| {
                for i in 0..count {
                    let seed = hash_u32(i as u32);
                    let angle = ((seed % 6284) as f64) / 1000.0; // 0..2π-ish
                    let speed = 0.6 + ((hash_u32(seed) % 100) as f64) / 100.0; // 0.6..1.6
                    let cycle = 3.0 / speed; // faster stars finish their sweep sooner
                    let phase =
                        (((hash_u32(seed.wrapping_add(7)) % 1000) as f64) / 1000.0) * cycle;

                    let local_t = (t * speed + phase).rem_euclid(cycle);
                    let frac = local_t / cycle; // 0..1 across one outward sweep
                    let dist = frac * frac * bound_x; // quadratic => accelerating outward

                    let x = angle.cos() * dist;
                    let y = angle.sin() * dist * (bound_y / bound_x);
                    let color = self.dim(frac);

                    ctx.draw(&Points { coords: &[(x, y)], color });
                }
            });

        frame.render_widget(canvas, area);
    }
}
