use ratatui::{
    layout::Rect,
    style::Color,
    symbols::Marker,
    widgets::canvas::{Canvas, Line as CanvasLine},
    Frame,
};

use crate::tui::types::THEMES;

const VERTS: [[f64; 3]; 8] = [
    [-1.0, -1.0, -1.0], [1.0, -1.0, -1.0], [1.0, 1.0, -1.0], [-1.0, 1.0, -1.0],
    [-1.0, -1.0,  1.0], [1.0, -1.0,  1.0], [1.0, 1.0,  1.0], [-1.0, 1.0,  1.0],
];

const EDGES: [(usize, usize); 12] = [
    (0, 1), (1, 2), (2, 3), (3, 0), // back face
    (4, 5), (5, 6), (6, 7), (7, 4), // front face
    (0, 4), (1, 5), (2, 6), (3, 7), // connectors
];

pub struct SpinningCube {
    pub scale: f64,
    pub color: Color,
}

impl SpinningCube {
    pub fn new() -> Self {
        Self {
            scale: 25.0,
            color: THEMES[0].primary,
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, t: f64) {
        let (ax, ay, az) = (t * 0.6, t * 0.9, t * 0.3);
        let rotated: Vec<[f64; 3]> = VERTS.iter().map(|v| rotate(*v, ax, ay, az)).collect();
        let projected: Vec<(f64, f64)> = rotated
            .iter()
            .map(|v| project(v[0], v[1], v[2], self.scale))
            .collect();

        let max_extent = projected
            .iter()
            .fold(0.0_f64, |m, (x, y)| m.max(x.abs()).max(y.abs()))
            .max(1.0);
        let half_x = max_extent * 1.25;

        let w = area.width.max(1) as f64;
        let h = area.height.max(1) as f64;
        let half_y = half_x * (2.0 * h) / w; // compensate for cell dot ratio

        let canvas = Canvas::default()
            .marker(Marker::Braille)
            .x_bounds([-half_x, half_x])
            .y_bounds([-half_y, half_y])
            .paint(|ctx| {
                for (a, b) in EDGES {
                    let (px1, py1) = projected[a];
                    let (px2, py2) = projected[b];
                    ctx.draw(&CanvasLine {
                        x1: px1,
                        y1: py1,
                        x2: px2,
                        y2: py2,
                        color: self.color,
                    });
                }
            });

        frame.render_widget(canvas, area);
    }
}

fn rotate(v: [f64; 3], ax: f64, ay: f64, az: f64) -> [f64; 3] {
    let (x, y, z) = (v[0], v[1], v[2]);
    // rotate around X
    let (y, z) = (y * ax.cos() - z * ax.sin(), y * ax.sin() + z * ax.cos());
    // rotate around Y
    let (x, z) = (x * ay.cos() + z * ay.sin(), -x * ay.sin() + z * ay.cos());
    // rotate around Z
    let (x, y) = (x * az.cos() - y * az.sin(), x * az.sin() + y * az.cos());
    [x, y, z]
}

fn project(x: f64, y: f64, z: f64, scale: f64) -> (f64, f64) {
    let d = 4.0; // camera distance
    let factor = scale / (d - z);
    (x * factor, y * factor)
}