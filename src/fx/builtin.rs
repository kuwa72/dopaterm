//! Built-in effects with danmaku / bullet-hell visual punch.

use super::{
    Effect, FxEvent, Instance, Particle, Parts, SHAPE_BOLT, SHAPE_DIAMOND, SHAPE_DISC, SHAPE_GLOW,
    SHAPE_RECT, SHAPE_RING, SHAPE_STAR,
};
use crate::input::KeyKind;

fn rand(seed: &mut u64) -> f32 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    (((*seed >> 8) as u32) & 0x00FF_FFFF) as f32 / ((1u32 << 24) as f32)
}

fn spread(seed: &mut u64, mag: f32) -> f32 {
    (rand(seed) * 2.0 - 1.0) * mag
}

fn rand_vec(seed: &mut u64, mag: f32) -> (f32, f32) {
    let a = rand(seed) * std::f32::consts::TAU;
    let v = rand(seed) * mag;
    (a.cos() * v, a.sin() * v)
}

fn hsv(seed: &mut u64, s: f32, v: f32) -> [f32; 4] {
    let h = rand(seed);
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);
    let (r, g, b) = match (i as i32) % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    [r, g, b, 1.0]
}

fn brighten(c: [f32; 4], k: f32) -> [f32; 4] {
    [
        c[0].mul_add(k, 0.0).min(1.0),
        c[1].mul_add(k, 0.0).min(1.0),
        c[2].mul_add(k, 0.0).min(1.0),
        c[3],
    ]
}

fn key_color(ch: Option<char>) -> [f32; 4] {
    match ch {
        Some(c) if c.is_ascii_digit() => [0.35, 0.95, 1.0, 1.0],
        Some(c) if c.is_ascii_punctuation() => [1.0, 1.0, 1.0, 1.0],
        Some(c)
            if ('\u{3040}'..='\u{309f}').contains(&c)
                || ('\u{30a0}'..='\u{30ff}').contains(&c)
                || ('\u{4e00}'..='\u{9fff}').contains(&c) =>
        {
            [0.85, 0.45, 1.0, 1.0]
        }
        Some(c) if c.is_uppercase() => [1.0, 0.55, 0.35, 1.0],
        Some(_) => [0.35, 1.0, 0.65, 1.0],
        None => [1.0, 0.9, 0.4, 1.0],
    }
}

fn spawn_spark(
    parts: &mut Parts,
    rng: &mut u64,
    x: f32,
    y: f32,
    color: [f32; 4],
    speed: f32,
    life: f32,
    kind: u32,
) {
    let (vx, vy) = rand_vec(rng, speed);
    parts.spawn(Particle {
        x,
        y,
        vx,
        vy: vy - speed * 0.4,
        ay: 500.0,
        life,
        max_life: life,
        size: 1.5 + rand(rng) * 2.5,
        color,
        kind,
        drag: 0.4 + rand(rng) * 1.0,
        spin: kind == SHAPE_STAR || kind == SHAPE_DIAMOND,
    });
}

fn spawn_comet_tail(
    parts: &mut Parts,
    rng: &mut u64,
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    color: [f32; 4],
    len: usize,
) {
    for i in 0..len {
        let t = i as f32 / len.max(1) as f32;
        parts.spawn(Particle {
            x: x - vx * t * 0.015,
            y: y - vy * t * 0.015,
            vx: vx * 0.1 + spread(rng, 10.0),
            vy: vy * 0.1 + spread(rng, 10.0),
            ay: 0.0,
            life: 0.12 + t * 0.18,
            max_life: 0.12 + t * 0.18,
            size: 2.5 * (1.0 - t * 0.6),
            color: [color[0], color[1], color[2], color[3] * (1.0 - t)],
            kind: SHAPE_GLOW,
            drag: 2.0,
            spin: false,
        });
    }
}

fn spawn_burst(
    parts: &mut Parts,
    rng: &mut u64,
    x: f32,
    y: f32,
    n: usize,
    color: [f32; 4],
    speed: f32,
    life: f32,
) {
    let shapes = [
        SHAPE_GLOW,
        SHAPE_GLOW,
        SHAPE_STAR,
        SHAPE_DIAMOND,
        SHAPE_DISC,
        SHAPE_RING,
    ];
    for _ in 0..n {
        let a = rand(rng) * std::f32::consts::TAU;
        let v = speed * (0.5 + rand(rng));
        let kind = shapes[(rand(rng) * shapes.len() as f32) as usize % shapes.len()];
        let mut c = color;
        if kind == SHAPE_RING {
            c[3] *= 0.55;
        }
        parts.spawn(Particle {
            x,
            y,
            vx: a.cos() * v,
            vy: a.sin() * v,
            ay: 180.0,
            life: life * (0.7 + rand(rng) * 0.6),
            max_life: life * (0.7 + rand(rng) * 0.6),
            size: 2.0 + rand(rng) * 3.5,
            color: c,
            kind,
            drag: 0.8 + rand(rng),
            spin: kind == SHAPE_STAR || kind == SHAPE_DIAMOND,
        });
    }
}

/// Keystroke: colored sparks plus an expanding ring keyed by the typed character.
pub struct Sparks {
    parts: Parts,
    rng: u64,
}

impl Sparks {
    pub fn new() -> Self {
        Self {
            parts: Parts::default(),
            rng: 0x9e3779b97f4a7c15,
        }
    }
}

impl Effect for Sparks {
    fn name(&self) -> &str {
        "sparks"
    }

    fn on_event(&mut self, ev: &FxEvent, scale: f32) {
        let FxEvent::Key { kind, ch, x, y } = ev else {
            return;
        };
        let color = match kind {
            KeyKind::Char => key_color(*ch),
            KeyKind::Backspace => [1.0, 0.35, 0.45, 1.0],
            KeyKind::Enter => [1.0, 0.85, 0.25, 1.0],
            KeyKind::Arrow => [0.55, 0.75, 1.0, 1.0],
            KeyKind::Other => [0.75, 0.75, 0.85, 1.0],
        };
        let n = (18.0 * scale).round() as usize;
        for _ in 0..n {
            let life = 0.18 + rand(&mut self.rng) * 0.28;
            spawn_spark(
                &mut self.parts,
                &mut self.rng,
                *x,
                *y,
                color,
                280.0,
                life,
                SHAPE_GLOW,
            );
        }
        // A crisp expanding ring for tactile feedback.
        self.parts.spawn(Particle {
            x: *x,
            y: *y,
            vx: 0.0,
            vy: 0.0,
            ay: 0.0,
            life: 0.28 * scale.max(0.4),
            max_life: 0.28 * scale.max(0.4),
            size: 8.0 + scale * 6.0,
            color: [color[0], color[1], color[2], 0.9],
            kind: SHAPE_RING,
            drag: 0.0,
            spin: false,
        });
    }

    fn update(&mut self, dt: f32) {
        self.parts.step(dt);
    }
    fn collect(&self, out: &mut Vec<Instance>) {
        self.parts.collect(out);
    }
    fn active(&self) -> bool {
        self.parts.alive()
    }
}

/// Erased characters explode into shards, a shockwave, and hot sparks.
pub struct Shatter {
    parts: Parts,
    rng: u64,
}

impl Shatter {
    pub fn new() -> Self {
        Self {
            parts: Parts::default(),
            rng: 0x85ebca6b,
        }
    }
}

impl Effect for Shatter {
    fn name(&self) -> &str {
        "shatter"
    }

    fn on_event(&mut self, ev: &FxEvent, scale: f32) {
        let FxEvent::Erased {
            x, y, w, h, color, ..
        } = ev
        else {
            return;
        };
        let n = (38.0 * scale).round() as usize;
        let c = brighten(*color, 1.5);
        let shapes = [
            SHAPE_RECT,
            SHAPE_RECT,
            SHAPE_DIAMOND,
            SHAPE_STAR,
            SHAPE_BOLT,
        ];
        for _ in 0..n {
            let life = 0.45 + rand(&mut self.rng) * 0.55;
            let size = if rand(&mut self.rng) < 0.25 {
                (*h / 2.2) + rand(&mut self.rng) * (*h / 1.8)
            } else {
                3.0 + rand(&mut self.rng) * (*h / 2.8).max(4.0)
            };
            let kind = shapes[(rand(&mut self.rng) * shapes.len() as f32) as usize % shapes.len()];
            let (vx, vy) = rand_vec(&mut self.rng, 420.0);
            self.parts.spawn(Particle {
                x: x + spread(&mut self.rng, w / 2.0),
                y: y + spread(&mut self.rng, h / 2.0),
                vx,
                vy: vy - 80.0 - rand(&mut self.rng) * 200.0,
                ay: 1000.0,
                life,
                max_life: life,
                size,
                color: c,
                kind,
                drag: 0.5 + rand(&mut self.rng) * 0.6,
                spin: true,
            });
        }
        // Shockwave ring.
        self.parts.spawn(Particle {
            x: *x,
            y: *y,
            vx: 0.0,
            vy: 0.0,
            ay: 0.0,
            life: 0.35 * scale.max(0.5),
            max_life: 0.35 * scale.max(0.5),
            size: h.max(*w) * 1.2,
            color: [c[0], c[1], c[2], 0.75],
            kind: SHAPE_RING,
            drag: 0.0,
            spin: false,
        });
        // Hot sparks.
        for _ in 0..(8.0 * scale).round() as usize {
            let g = 0.5 + rand(&mut self.rng) * 0.4;
            let life = 0.25 + rand(&mut self.rng) * 0.2;
            spawn_spark(
                &mut self.parts,
                &mut self.rng,
                *x,
                *y,
                [1.0, g, 0.2, 1.0],
                220.0,
                life,
                SHAPE_GLOW,
            );
        }
    }

    fn update(&mut self, dt: f32) {
        self.parts.step(dt);
    }
    fn collect(&self, out: &mut Vec<Instance>) {
        self.parts.collect(out);
    }
    fn active(&self) -> bool {
        self.parts.alive()
    }
}

const FIREWORK_COLORS: [[f32; 4]; 5] = [
    [1.0, 0.35, 0.35, 1.0],
    [0.4, 0.9, 1.0, 1.0],
    [1.0, 0.85, 0.3, 1.0],
    [0.75, 0.45, 1.0, 1.0],
    [0.45, 1.0, 0.55, 1.0],
];

/// Command-finished celebration: rockets rise from the cursor, then burst twice.
pub struct Fireworks {
    parts: Parts,
    rng: u64,
    /// Pending bursts: (fuse_seconds, x, y, color).
    pending: Vec<(f32, f32, f32, [f32; 4])>,
}

impl Fireworks {
    pub fn new() -> Self {
        Self {
            parts: Parts::default(),
            rng: 0x27d4eb2f,
            pending: Vec::new(),
        }
    }
}

impl Effect for Fireworks {
    fn name(&self) -> &str {
        "fireworks"
    }

    fn on_event(&mut self, ev: &FxEvent, scale: f32) {
        let FxEvent::CommandDone { x, y } = ev else {
            return;
        };
        let n = (6.0 * scale).round().max(1.0) as usize;
        for i in 0..n {
            let color = FIREWORK_COLORS[(rand(&mut self.rng) * FIREWORK_COLORS.len() as f32)
                as usize
                % FIREWORK_COLORS.len()];
            let tx = x + spread(&mut self.rng, 280.0);
            let ty = y - 160.0 - rand(&mut self.rng) * 220.0;
            let fuse = 0.26 + i as f32 * 0.18 + rand(&mut self.rng) * 0.12;
            let vx = (tx - x) / fuse;
            let vy = (ty - y) / fuse;
            self.parts.spawn(Particle {
                x: *x,
                y: *y,
                vx,
                vy,
                ay: 0.0,
                life: fuse,
                max_life: fuse,
                size: 6.0,
                color: [1.0, 0.97, 0.85, 0.95],
                kind: SHAPE_GLOW,
                drag: 0.0,
                spin: false,
            });
            spawn_comet_tail(
                &mut self.parts,
                &mut self.rng,
                *x,
                *y,
                vx,
                vy,
                [1.0, 0.97, 0.85, 0.7],
                10,
            );
            self.pending.push((fuse, tx, ty, color));
        }
    }

    fn update(&mut self, dt: f32) {
        self.parts.step(dt);
        let mut bursts = Vec::new();
        self.pending.retain_mut(|p| {
            p.0 -= dt;
            if p.0 <= 0.0 {
                bursts.push((p.1, p.2, p.3));
                false
            } else {
                true
            }
        });
        for (bx, by, color) in bursts {
            spawn_burst(
                &mut self.parts,
                &mut self.rng,
                bx,
                by,
                54,
                color,
                190.0,
                0.75,
            );
            // Secondary micro-bursts for extra glitter.
            if rand(&mut self.rng) < 0.65 {
                let mx = bx + spread(&mut self.rng, 45.0);
                let my = by + spread(&mut self.rng, 45.0);
                spawn_burst(
                    &mut self.parts,
                    &mut self.rng,
                    mx,
                    my,
                    22,
                    brighten(color, 1.2),
                    95.0,
                    0.45,
                );
            }
        }
    }

    fn collect(&self, out: &mut Vec<Instance>) {
        self.parts.collect(out);
    }
    fn active(&self) -> bool {
        self.parts.alive() || !self.pending.is_empty()
    }
}

const CONFETTI_COLORS: [[f32; 4]; 6] = [
    [1.0, 0.42, 0.55, 1.0],
    [1.0, 0.8, 0.25, 1.0],
    [0.35, 0.85, 1.0, 1.0],
    [0.55, 1.0, 0.45, 1.0],
    [0.75, 0.5, 1.0, 1.0],
    [1.0, 0.6, 0.3, 1.0],
];

/// Enter: a vertical beam, expanding rings, and paper confetti.
pub struct Confetti {
    parts: Parts,
    rng: u64,
}

impl Confetti {
    pub fn new() -> Self {
        Self {
            parts: Parts::default(),
            rng: 0x51ed270b,
        }
    }
}

impl Effect for Confetti {
    fn name(&self) -> &str {
        "confetti"
    }

    fn on_event(&mut self, ev: &FxEvent, scale: f32) {
        let FxEvent::Confetti { x, y, w, h } = ev else {
            return;
        };
        // Vertical beam through the cursor line.
        self.parts.spawn(Particle {
            x: *x,
            y: *y - *h * 0.5,
            vx: 0.0,
            vy: 0.0,
            ay: 0.0,
            life: 0.22 * scale.max(0.5),
            max_life: 0.22 * scale.max(0.5),
            size: *h,
            color: [1.0, 0.9, 0.4, 0.45],
            kind: SHAPE_GLOW,
            drag: 0.0,
            spin: false,
        });
        // Expanding rings from the cursor.
        for i in 0..3 {
            self.parts.spawn(Particle {
                x: *x,
                y: *y,
                vx: 0.0,
                vy: 0.0,
                ay: 0.0,
                life: 0.35 + i as f32 * 0.08,
                max_life: 0.35 + i as f32 * 0.08,
                size: 12.0 + i as f32 * 10.0,
                color: [1.0, 1.0, 1.0, 0.35 - i as f32 * 0.08],
                kind: SHAPE_RING,
                drag: 0.0,
                spin: false,
            });
        }
        // Confetti from the bottom edge.
        let n = (72.0 * scale).round() as usize;
        let shapes = [SHAPE_RECT, SHAPE_STAR, SHAPE_DIAMOND, SHAPE_DIAMOND];
        for i in 0..n {
            let life = 1.0 + rand(&mut self.rng) * 1.0;
            let kind = shapes[i % shapes.len()];
            self.parts.spawn(Particle {
                x: rand(&mut self.rng) * *w,
                y: *h + 8.0,
                vx: spread(&mut self.rng, 120.0),
                vy: -(700.0 + rand(&mut self.rng) * 600.0),
                ay: 900.0,
                life,
                max_life: life,
                size: 3.0 + rand(&mut self.rng) * 4.0,
                color: CONFETTI_COLORS[i % CONFETTI_COLORS.len()],
                kind,
                drag: 0.25,
                spin: true,
            });
        }
    }

    fn update(&mut self, dt: f32) {
        self.parts.step(dt);
    }
    fn collect(&self, out: &mut Vec<Instance>) {
        self.parts.collect(out);
    }
    fn active(&self) -> bool {
        self.parts.alive()
    }
}

/// Cursor jumps leave a warp trail and a brief comet tail.
pub struct CursorTrail {
    parts: Parts,
    rng: u64,
}

impl CursorTrail {
    pub fn new() -> Self {
        Self {
            parts: Parts::default(),
            rng: 0x1f0d3a77,
        }
    }
}

impl Effect for CursorTrail {
    fn name(&self) -> &str {
        "cursor_trail"
    }

    fn on_event(&mut self, ev: &FxEvent, scale: f32) {
        let FxEvent::CursorMoved { x, y, w, h, dx, dy } = ev else {
            return;
        };
        let len = (dx.hypot(*dy) * 0.015).clamp(4.0, 18.0) * scale;
        let vx = *dx * 8.0;
        let vy = *dy * 8.0;
        spawn_comet_tail(
            &mut self.parts,
            &mut self.rng,
            *x,
            *y,
            vx,
            vy,
            [0.55, 0.9, 1.0, 0.6],
            len as usize,
        );
        // Ghost rect at the departure cell.
        self.parts.spawn(Particle {
            x: *x,
            y: *y,
            vx: 0.0,
            vy: 0.0,
            ay: 0.0,
            life: 0.18 * scale.max(0.5),
            max_life: 0.18 * scale.max(0.5),
            size: h.max(*w) * 1.3,
            color: [0.55, 0.85, 1.0, 0.5],
            kind: SHAPE_RECT,
            drag: 0.0,
            spin: false,
        });
    }

    fn update(&mut self, dt: f32) {
        self.parts.step(dt);
    }
    fn collect(&self, out: &mut Vec<Instance>) {
        self.parts.collect(out);
    }
    fn active(&self) -> bool {
        self.parts.alive()
    }
}

/// Bell: a white flash and red shockwave at the cursor.
pub struct BellFlash {
    parts: Parts,
    rng: u64,
}

impl BellFlash {
    pub fn new() -> Self {
        Self {
            parts: Parts::default(),
            rng: 0xbadc0ffee,
        }
    }
}

impl Effect for BellFlash {
    fn name(&self) -> &str {
        "bell_flash"
    }

    fn on_event(&mut self, ev: &FxEvent, scale: f32) {
        let FxEvent::Bell { x, y, w, h } = ev else {
            return;
        };
        let base = h.max(*w) * 2.0;
        self.parts.spawn(Particle {
            x: *x,
            y: *y,
            vx: 0.0,
            vy: 0.0,
            ay: 0.0,
            life: 0.12 * scale.max(0.5),
            max_life: 0.12 * scale.max(0.5),
            size: base * 5.0,
            color: [1.0, 1.0, 1.0, 0.65],
            kind: SHAPE_GLOW,
            drag: 0.0,
            spin: false,
        });
        for i in 0..2 {
            self.parts.spawn(Particle {
                x: *x,
                y: *y,
                vx: 0.0,
                vy: 0.0,
                ay: 0.0,
                life: 0.25 + i as f32 * 0.08,
                max_life: 0.25 + i as f32 * 0.08,
                size: base * (1.2 + i as f32 * 0.7),
                color: [1.0, 0.25, 0.25, 0.5 - i as f32 * 0.1],
                kind: SHAPE_RING,
                drag: 0.0,
                spin: false,
            });
        }
        for _ in 0..(6.0 * scale).round() as usize {
            let (vx, vy) = rand_vec(&mut self.rng, 200.0);
            self.parts.spawn(Particle {
                x: *x,
                y: *y,
                vx,
                vy,
                ay: 300.0,
                life: 0.22 + rand(&mut self.rng) * 0.15,
                max_life: 0.22 + rand(&mut self.rng) * 0.15,
                size: 2.0 + rand(&mut self.rng) * 2.0,
                color: [1.0, 0.35 + rand(&mut self.rng) * 0.3, 0.2, 1.0],
                kind: if rand(&mut self.rng) < 0.5 {
                    SHAPE_BOLT
                } else {
                    SHAPE_GLOW
                },
                drag: 0.8,
                spin: true,
            });
        }
    }

    fn update(&mut self, dt: f32) {
        self.parts.step(dt);
    }
    fn collect(&self, out: &mut Vec<Instance>) {
        self.parts.collect(out);
    }
    fn active(&self) -> bool {
        self.parts.alive()
    }
}

/// Breathing aura plus drifting stars while the terminal waits for input.
pub struct WaitPulse {
    parts: Parts,
    rng: u64,
}

impl WaitPulse {
    pub fn new() -> Self {
        Self {
            parts: Parts::default(),
            rng: 0xcafebabe,
        }
    }
}

impl Effect for WaitPulse {
    fn name(&self) -> &str {
        "wait_pulse"
    }

    fn on_event(&mut self, ev: &FxEvent, scale: f32) {
        let FxEvent::Waiting { x, y } = ev else {
            return;
        };
        self.parts.spawn(Particle {
            x: *x,
            y: *y,
            vx: 0.0,
            vy: 0.0,
            ay: 0.0,
            life: 1.4 * scale.max(0.4),
            max_life: 1.4 * scale.max(0.4),
            size: 40.0,
            color: [1.0, 0.72, 0.25, 0.4],
            kind: SHAPE_GLOW,
            drag: 0.0,
            spin: false,
        });
        self.parts.spawn(Particle {
            x: *x,
            y: *y,
            vx: 0.0,
            vy: 0.0,
            ay: 0.0,
            life: 1.1 * scale.max(0.4),
            max_life: 1.1 * scale.max(0.4),
            size: 28.0,
            color: [1.0, 0.9, 0.5, 0.35],
            kind: SHAPE_RING,
            drag: 0.0,
            spin: false,
        });
        for _ in 0..(5.0 * scale).round() as usize {
            let a = rand(&mut self.rng) * std::f32::consts::TAU;
            let r = 35.0 + rand(&mut self.rng) * 25.0;
            let (vx, vy) = rand_vec(&mut self.rng, 18.0);
            self.parts.spawn(Particle {
                x: x + a.cos() * r,
                y: y + a.sin() * r,
                vx,
                vy,
                ay: 0.0,
                life: 1.2 + rand(&mut self.rng) * 0.6,
                max_life: 1.2 + rand(&mut self.rng) * 0.6,
                size: 1.5 + rand(&mut self.rng) * 2.0,
                color: hsv(&mut self.rng, 0.6, 1.0),
                kind: SHAPE_STAR,
                drag: 0.2,
                spin: true,
            });
        }
    }

    fn update(&mut self, dt: f32) {
        self.parts.step(dt);
    }
    fn collect(&self, out: &mut Vec<Instance>) {
        self.parts.collect(out);
    }
    fn active(&self) -> bool {
        self.parts.alive()
    }
}

/// PTY output spawns shooting stars from the right edge.
pub struct OutputRain {
    parts: Parts,
    rng: u64,
    budget: f32,
    win_w: f32,
    win_h: f32,
}

impl OutputRain {
    pub fn new() -> Self {
        Self {
            parts: Parts::default(),
            rng: 0xdeadbabe,
            budget: 0.0,
            win_w: 960.0,
            win_h: 600.0,
        }
    }
}

impl Effect for OutputRain {
    fn name(&self) -> &str {
        "output_rain"
    }

    fn on_event(&mut self, ev: &FxEvent, scale: f32) {
        if let FxEvent::PtyOutput { n, w, h } = ev {
            self.budget += *n as f32 * 0.12 * scale;
            self.win_w = *w;
            self.win_h = *h;
        }
    }

    fn update(&mut self, dt: f32) {
        self.parts.step(dt);
        let cap = (self.budget as usize).min(10);
        for _ in 0..cap {
            self.budget -= 1.0;
            let y = rand(&mut self.rng) * self.win_h;
            let speed = 320.0 + rand(&mut self.rng) * 340.0;
            let color = hsv(&mut self.rng, 0.55, 1.0);
            let x = self.win_w + 20.0;
            self.parts.spawn(Particle {
                x,
                y,
                vx: -speed,
                vy: rand(&mut self.rng) * 80.0,
                ay: 0.0,
                life: 0.8 + rand(&mut self.rng) * 0.6,
                max_life: 0.8 + rand(&mut self.rng) * 0.6,
                size: 2.0 + rand(&mut self.rng) * 2.5,
                color,
                kind: SHAPE_GLOW,
                drag: 0.0,
                spin: false,
            });
            spawn_comet_tail(&mut self.parts, &mut self.rng, x, y, -speed, 0.0, color, 6);
        }
    }

    fn collect(&self, out: &mut Vec<Instance>) {
        self.parts.collect(out);
    }
    fn active(&self) -> bool {
        self.parts.alive() || self.budget > 0.0
    }
}

/// Child process error exit: an angle-grinder shower of orange sparks arcing
/// downward and outward from the contact point, dying before the floor.
pub struct ProcessError {
    parts: Parts,
    rng: u64,
    floor: f32,
}

impl ProcessError {
    pub fn new() -> Self {
        Self {
            parts: Parts::default(),
            rng: 0xdeadbeef,
            floor: 2000.0,
        }
    }
}

impl Effect for ProcessError {
    fn name(&self) -> &str {
        "process_error"
    }

    fn on_event(&mut self, ev: &FxEvent, scale: f32) {
        let FxEvent::ChildError {
            x, y, h, status, ..
        } = ev
        else {
            return;
        };
        let cx = *x;
        let cy = *y;
        self.floor = *h + 20.0;
        let mag = (1.0 + (*status as f32).abs() / 64.0).min(3.0) * scale;

        // Hard practical light flickering at the grinder contact point.
        for _ in 0..(12.0 * mag).round() as usize {
            self.parts.spawn(Particle {
                x: cx + spread(&mut self.rng, 2.0),
                y: cy + spread(&mut self.rng, 2.0),
                vx: 0.0,
                vy: 0.0,
                ay: 0.0,
                life: (0.03 + rand(&mut self.rng) * 0.07) * mag,
                max_life: (0.03 + rand(&mut self.rng) * 0.07) * mag,
                size: 10.0 + rand(&mut self.rng) * 22.0,
                color: [1.0, 0.78, 0.35, 0.4 + rand(&mut self.rng) * 0.4],
                kind: SHAPE_GLOW,
                drag: 0.0,
                spin: false,
            });
        }

        // Dense spark stream: shallow downward/outward fan below the contact point.
        let n = (240.0 * mag).round() as usize;
        for _ in 0..n {
            let spread_a = spread(&mut self.rng, 1.0).clamp(-1.35, 1.35);
            let angle = std::f32::consts::FRAC_PI_2 + spread_a;
            let v = 300.0 + rand(&mut self.rng) * 420.0;
            let vx = angle.cos() * v;
            let vy = angle.sin() * v.abs();
            let life = 0.8 + rand(&mut self.rng) * 1.2;
            let color = if rand(&mut self.rng) < 0.18 {
                [1.0, 0.96, 0.82, 1.0]
            } else if rand(&mut self.rng) < 0.55 {
                [1.0, 0.72, 0.18, 1.0]
            } else {
                [1.0, 0.42, 0.06, 1.0]
            };
            self.parts.spawn(Particle {
                x: cx + spread(&mut self.rng, 3.0),
                y: cy + spread(&mut self.rng, 2.0),
                vx,
                vy,
                ay: 650.0,
                life: life * mag,
                max_life: life * mag,
                size: 2.2 + rand(&mut self.rng) * 3.2,
                color,
                kind: if rand(&mut self.rng) < 0.45 {
                    SHAPE_RECT
                } else {
                    SHAPE_GLOW
                },
                drag: 0.1 + rand(&mut self.rng) * 0.4,
                spin: true,
            });
        }

        // Larger bright core sparks near the contact point.
        for _ in 0..(30.0 * mag).round() as usize {
            let spread_a = spread(&mut self.rng, 0.75).clamp(-1.1, 1.1);
            let angle = std::f32::consts::FRAC_PI_2 + spread_a;
            let v = 180.0 + rand(&mut self.rng) * 260.0;
            self.parts.spawn(Particle {
                x: cx + spread(&mut self.rng, 2.0),
                y: cy + spread(&mut self.rng, 1.5),
                vx: angle.cos() * v,
                vy: angle.sin() * v.abs(),
                ay: 600.0,
                life: (0.5 + rand(&mut self.rng) * 0.7) * mag,
                max_life: (0.5 + rand(&mut self.rng) * 0.7) * mag,
                size: 3.0 + rand(&mut self.rng) * 3.5,
                color: [1.0, 0.88, 0.35, 1.0],
                kind: SHAPE_GLOW,
                drag: 0.2,
                spin: false,
            });
        }
    }

    fn update(&mut self, dt: f32) {
        self.parts.step(dt);
        // Sparks die as soon as they pass the bottom edge (floor).
        for p in &mut self.parts.0 {
            if p.y > self.floor {
                p.life = 0.0;
            }
        }
    }
    fn collect(&self, out: &mut Vec<Instance>) {
        self.parts.collect(out);
    }
    fn active(&self) -> bool {
        self.parts.alive()
    }
}
