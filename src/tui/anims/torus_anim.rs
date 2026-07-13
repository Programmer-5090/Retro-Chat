use ratatui::{
    layout::Rect,
    style::Color,
    symbols::Marker,
    widgets::canvas::{ Canvas, Points },
    Frame,
};

const BIG_R: f64 = 1.6; // distance from the torus's center to the tube's center
const SMALL_R: f64 = 0.7; // tube radius
const U_STEPS: usize = 36; // around the big ring
const V_STEPS: usize = 14; // around the tube

pub struct SpinningTorus {
    pub scale: f64,
    pub color: Color,
}

impl SpinningTorus {
    pub fn new() -> Self {
        Self {
            scale: 20.0,
            color: Color::Rgb(255, 176, 0),
        }
    }

    fn dim(&self, brightness: f64) -> Color {
        let (r, g, b) = match self.color {
            Color::Rgb(r, g, b) => (r, g, b),
            _ => (255, 176, 0),
        };
        let f = 0.2 + brightness.clamp(0.0, 1.0) * 0.8;
        Color::Rgb(((r as f64) * f) as u8, ((g as f64) * f) as u8, ((b as f64) * f) as u8)
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, t: f64) {
        let (ax, ay) = (t * 0.7, t * 1.1);

        // Sample the torus surface and project each point.
        let mut projected: Vec<(f64, f64, f64)> = Vec::with_capacity(U_STEPS * V_STEPS);
        for iu in 0..U_STEPS {
            let u = ((iu as f64) / (U_STEPS as f64)) * std::f64::consts::TAU;
            let (su, cu) = u.sin_cos();
            for iv in 0..V_STEPS {
                let v = ((iv as f64) / (V_STEPS as f64)) * std::f64::consts::TAU;
                let (sv, cv) = v.sin_cos();
                let x = (BIG_R + SMALL_R * cv) * cu;
                let y = (BIG_R + SMALL_R * cv) * su;
                let z = SMALL_R * sv;

                let (rx, ry, rz) = rotate(x, y, z, ax, ay);
                let (px, py) = project(rx, ry, rz, self.scale);
                projected.push((px, py, rz));
            }
        }

        // Auto-fit the camera bounds to the torus's current projected extent
        let max_extent = projected
            .iter()
            .fold(0.0_f64, |m, (x, y, _)| m.max(x.abs()).max(y.abs()))
            .max(1.0);
        let half_x = max_extent * 1.2;

        let w = area.width.max(1) as f64;
        let h = area.height.max(1) as f64;
        let half_y = half_x * (2.0 * h) / w;

        let canvas = Canvas::default()
            .marker(Marker::Braille)
            .x_bounds([-half_x, half_x])
            .y_bounds([-half_y, half_y])
            .paint(|ctx| {
                for (px, py, depth) in &projected {
                    // Points further toward the camera read brighter, giving
                    // a cheap sense of the tube's roundness as it spins.
                    let shade = ((depth + SMALL_R) / (2.0 * SMALL_R)).clamp(0.1, 1.0);
                    ctx.draw(&Points { coords: &[(*px, *py)], color: self.dim(shade) });
                }
            });

        frame.render_widget(canvas, area);
    }
}

fn rotate(x: f64, y: f64, z: f64, ax: f64, ay: f64) -> (f64, f64, f64) {
    // rotate around X
    let (y, z) = (y * ax.cos() - z * ax.sin(), y * ax.sin() + z * ax.cos());
    // rotate around Y
    let (x, z) = (x * ay.cos() + z * ay.sin(), -x * ay.sin() + z * ay.cos());
    (x, y, z)
}

fn project(x: f64, y: f64, z: f64, scale: f64) -> (f64, f64) {
    let d = 5.0; // camera distance
    let factor = scale / (d - z);
    (x * factor, y * factor)
}
