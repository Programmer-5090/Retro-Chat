// cube_bg.rs — Rotating wireframe cube "watermark" widget for Retro-Chat

use ratatui::{
    layout::Rect,
    style::Color,
    symbols::Marker,
    widgets::canvas::{Canvas, Line as CanvasLine},
    Frame,
};

use crate::tui::types::THEMES;

/// 8 corners of a unit cube centered on the origin.
const VERTS: [[f64; 3]; 8] = [
    [-1.0, -1.0, -1.0], [1.0, -1.0, -1.0], [1.0, 1.0, -1.0], [-1.0, 1.0, -1.0],
    [-1.0, -1.0,  1.0], [1.0, -1.0,  1.0], [1.0, 1.0,  1.0], [-1.0, 1.0,  1.0],
];

/// Pairs of vertex indices that form the 12 edges of the cube.
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
            scale: 6.0,
            // dim amber so it reads as "background", not foreground content
            color: THEMES[0].primary,
        }
    }

    /// `t` is elapsed seconds — pull this from an `Instant` you already keep
    /// in your tick/app loop (e.g. `app.start_time.elapsed().as_secs_f64()`).
    pub fn render(&self, frame: &mut Frame, area: Rect, t: f64) {
        let (ax, ay, az) = (t * 0.6, t * 0.9, t * 0.3);
        let rotated: Vec<[f64; 3]> = VERTS.iter().map(|v| rotate(*v, ax, ay, az)).collect();

        let canvas = Canvas::default()
            .marker(Marker::Braille)
            .x_bounds([-10.0, 10.0])
            .y_bounds([-10.0, 10.0])
            .paint(|ctx| {
                for (a, b) in EDGES {
                    let (x1, y1, z1) = (rotated[a][0], rotated[a][1], rotated[a][2]);
                    let (x2, y2, z2) = (rotated[b][0], rotated[b][1], rotated[b][2]);
                    let (px1, py1) = project(x1, y1, z1, self.scale);
                    let (px2, py2) = project(x2, y2, z2, self.scale);
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
