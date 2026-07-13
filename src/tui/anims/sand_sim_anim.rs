use std::time::{ Duration, Instant };

use ratatui::{
    layout::Rect,
    style::{ Color, Style },
    text::{ Line, Span },
    widgets::Paragraph,
    Frame,
};

const STEP_INTERVAL: Duration = Duration::from_millis(55);
const SPAWN_GLYPH: &str = "\u{2591}"; // ░

pub struct SandSim {
    pub color: Color,
    grid: Vec<u8>,
    width: usize,
    height: usize,
    last_step: Instant,
    rng_state: u64,
}

impl SandSim {
    pub fn new() -> Self {
        Self {
            color: Color::Rgb(255, 176, 0),
            grid: Vec::new(),
            width: 0,
            height: 0,
            last_step: Instant::now(),
            rng_state: 0x2545f4914f6cdd1d,
        }
    }

    fn idx(&self, x: usize, y: usize) -> usize {
        y * self.width + x
    }

    fn next_rand(&mut self) -> u64 {
        // xorshift64
        let mut x = self.rng_state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng_state = x;
        x
    }

    fn ensure_size(&mut self, w: usize, h: usize) {
        if self.width != w || self.height != h {
            self.width = w;
            self.height = h;
            self.grid = vec![0u8; w * h];
        }
    }

    fn step(&mut self) {
        let (w, h) = (self.width, self.height);
        if w == 0 || h == 0 {
            return;
        }

        // Occasionally spawn a new grain at a random column along the top.
        if self.next_rand() % 3 == 0 {
            let col = (self.next_rand() as usize) % w;
            let i = self.idx(col, 0);
            self.grid[i] = 1;
        }

        // Update bottom-to-top so a grain never falls twice in one step.
        for y in (0..h.saturating_sub(1)).rev() {
            for x in 0..w {
                let i = self.idx(x, y);
                if self.grid[i] == 0 {
                    continue;
                }
                let below = self.idx(x, y + 1);
                if self.grid[below] == 0 {
                    self.grid.swap(i, below);
                    continue;
                }
                let go_left_first = self.next_rand() % 2 == 0;
                let dirs: [i32; 2] = if go_left_first { [-1, 1] } else { [1, -1] };
                for dx in dirs {
                    let nx = (x as i32) + dx;
                    if nx < 0 || (nx as usize) >= w {
                        continue;
                    }
                    let ni = self.idx(nx as usize, y + 1);
                    if self.grid[ni] == 0 {
                        self.grid.swap(i, ni);
                        break;
                    }
                }
            }
        }

        // Once the floor is completely packed, clear it so grains keep
        // falling forever instead of the whole box filling up and going
        // static.
        let bottom = h - 1;
        if (0..w).all(|x| self.grid[self.idx(x, bottom)] != 0) {
            for x in 0..w {
                let idx = self.idx(x, bottom);
                self.grid[idx] = 0;
            }
        }
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, _t: f64) {
        let w = area.width as usize;
        let h = area.height as usize;
        if w == 0 || h == 0 {
            return;
        }
        self.ensure_size(w, h);

        let now = Instant::now();
        if now.duration_since(self.last_step) >= STEP_INTERVAL {
            self.last_step = now;
            self.step();
        }

        let mut lines: Vec<Line> = Vec::with_capacity(h);
        for y in 0..h {
            let mut spans: Vec<Span> = Vec::with_capacity(w);
            for x in 0..w {
                if self.grid[self.idx(x, y)] != 0 {
                    spans.push(Span::styled(SPAWN_GLYPH, Style::default().fg(self.color)));
                } else {
                    spans.push(Span::raw(" "));
                }
            }
            lines.push(Line::from(spans));
        }

        let p = Paragraph::new(lines);
        frame.render_widget(p, area);
    }
}
