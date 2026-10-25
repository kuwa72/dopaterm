//! Built-in effects with danmaku / bullet-hell visual punch.

use super::{
    Effect, FxEvent, Instance, Particle, Parts, SHAPE_BEAM, SHAPE_BOLT, SHAPE_DIAMOND, SHAPE_DISC,
    SHAPE_GLOW, SHAPE_RECT, SHAPE_RING, SHAPE_STAR,
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
    hsv_at(rand(seed), s, v)
}

fn hsv_at(h: f32, s: f32, v: f32) -> [f32; 4] {
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

/// Dim a color without changing its hue; secondary shells stay behind the
/// white-hot impact in the value hierarchy.
fn with_alpha(c: [f32; 4], a: f32) -> [f32; 4] {
    [c[0], c[1], c[2], c[3] * a]
}

/// Theme-park tier: a hard step at 0.8 (0.5) ramping to 1.0 at max. Below 0.8
/// the dial stays in everyday range; 0.8+ deliberately goes over the top.
fn overdrive(scale: f32) -> f32 {
    if scale < 0.8 {
        return 0.0;
    }
    0.5 + 0.5 * ((scale - 0.8) / 0.2).clamp(0.0, 1.0)
}

/// Visible intensity stage for the 10-step dial (0 = off). Every 0.2 unlocks
/// one more layer, so 0.1..0.9 is not just "fewer of the same particles".
fn stage(scale: f32) -> u8 {
    match scale {
        s if s <= 0.0 => 0,
        s if s < 0.3 => 1,
        s if s < 0.5 => 2,
        s if s < 0.7 => 3,
        s if s < 0.9 => 4,
        _ => 5,
    }
}

/// Particle density curve. It spreads the ten steps apart and ramps harder at
/// the top, so each 0.1 is visible instead of a linear "slightly more".
fn density(scale: f32) -> f32 {
    let s = scale.clamp(0.0, 1.0);
    s * (0.75 + 0.65 * s)
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

/// Keystrokes closer together than this keep the combo alive.
const COMBO_WINDOW: f32 = 0.35;
/// Quiet time after which the combo resets.
const COMBO_RESET: f32 = 0.6;

/// Milestone reward for sustained typing: a white ring plus a star spray.
/// The color shifts with each milestone so the reward never looks identical.
fn spawn_combo_burst(parts: &mut Parts, rng: &mut u64, x: f32, y: f32, combo: u32, scale: f32) {
    let color = hsv(rng, 0.65, 1.0);
    parts.spawn(Particle {
        x,
        y,
        vx: 0.0,
        vy: 0.0,
        ay: 0.0,
        life: 0.34 * scale.max(0.4),
        max_life: 0.34 * scale.max(0.4),
        size: 14.0 + (combo as f32).sqrt() * 2.0,
        color: [1.0, 1.0, 1.0, 0.9],
        kind: SHAPE_RING,
        drag: 0.0,
        spin: false,
    });
    let n = (7.0 * density(scale)).round().max(2.0) as usize;
    for i in 0..n {
        let a = i as f32 * std::f32::consts::TAU / n as f32 + spread(rng, 0.2);
        let v = 130.0 + rand(rng) * 60.0;
        let life = 0.3 + rand(rng) * 0.15;
        parts.spawn(Particle {
            x,
            y,
            vx: a.cos() * v,
            vy: a.sin() * v - 40.0,
            ay: 220.0,
            life,
            max_life: life,
            size: 2.5 + rand(rng) * 2.0,
            color,
            kind: SHAPE_STAR,
            drag: 1.0,
            spin: true,
        });
    }
}

/// Keystroke: colored sparks plus an expanding ring keyed by the typed character.
pub struct Sparks {
    parts: Parts,
    rng: u64,
    /// Consecutive keystrokes; resets after a quiet beat.
    combo: u32,
    /// Seconds since the last keystroke.
    idle: f32,
}

impl Sparks {
    pub fn new() -> Self {
        Self {
            parts: Parts::default(),
            rng: 0x9e3779b97f4a7c15,
            combo: 0,
            idle: COMBO_RESET,
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
        self.combo = if self.idle <= COMBO_WINDOW {
            self.combo + 1
        } else {
            1
        };
        self.idle = 0.0;
        let od = overdrive(scale);
        let st = stage(scale);
        // Theme-park tier: keystrokes turn into rainbow sparks.
        let color = if od > 0.5 {
            hsv(&mut self.rng, 0.7, 1.0)
        } else {
            match kind {
                KeyKind::Char => key_color(*ch),
                KeyKind::Backspace => [1.0, 0.35, 0.45, 1.0],
                KeyKind::Enter => [1.0, 0.85, 0.25, 1.0],
                KeyKind::Arrow => [0.55, 0.75, 1.0, 1.0],
                KeyKind::Other => [0.75, 0.75, 0.85, 1.0],
            }
        };
        let n = ((18.0 + 8.0 * od) * density(scale)).round() as usize;
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
            size: 8.0 + density(scale) * 6.0 + od * 4.0,
            color: [color[0], color[1], color[2], 0.9],
            kind: SHAPE_RING,
            drag: 0.0,
            spin: false,
        });
        // Milestones get closer as the dial rises: 50, then 25, then 10.
        let step = match st {
            5 => 10,
            3 | 4 => 25,
            _ => 50,
        };
        if self.combo % step == 0 {
            spawn_combo_burst(&mut self.parts, &mut self.rng, *x, *y, self.combo, scale);
        }
    }

    fn update(&mut self, dt: f32) {
        self.parts.step(dt);
        if self.idle < COMBO_RESET {
            self.idle += dt;
        }
        if self.idle >= COMBO_RESET {
            self.combo = 0;
        }
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
    /// Impacts seen; every 4th gets a "nova" at stage 5.
    hits: u32,
}

impl Shatter {
    pub fn new() -> Self {
        Self {
            parts: Parts::default(),
            rng: 0x85ebca6b,
            hits: 0,
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
        self.hits = self.hits.wrapping_add(1);
        let n = (38.0 * density(scale)).round() as usize;
        let st = stage(scale);
        let od = overdrive(scale);
        let nova = st >= 5 && self.hits % 4 == 0;
        let c = brighten(*color, 1.5);
        // Glass-like shards, not cardboard rectangles: half the burst is
        // evenly spaced so it reads as a designed shatter, half is chaotic.
        let shapes = [
            SHAPE_DIAMOND,
            SHAPE_DIAMOND,
            SHAPE_STAR,
            SHAPE_BOLT,
            SHAPE_RECT,
        ];
        for i in 0..n {
            let life = 0.45 + rand(&mut self.rng) * 0.55;
            let size = if rand(&mut self.rng) < 0.25 {
                11.0 + rand(&mut self.rng) * (*h / 1.6).max(10.0)
            } else {
                4.0 + rand(&mut self.rng) * (*h / 2.0).max(6.0)
            };
            let structured = i % 2 == 0;
            let angle = if structured {
                i as f32 * std::f32::consts::TAU / n as f32 + spread(&mut self.rng, 0.16)
            } else {
                rand(&mut self.rng) * std::f32::consts::TAU
            };
            let speed = if structured {
                260.0 + rand(&mut self.rng) * 140.0
            } else {
                120.0 + rand(&mut self.rng) * 420.0
            };
            let kind = if structured {
                [SHAPE_DIAMOND, SHAPE_STAR, SHAPE_BOLT][i % 3]
            } else {
                shapes[(rand(&mut self.rng) * shapes.len() as f32) as usize % shapes.len()]
            };
            // At 0.8+ a quarter of the shards turn rainbow.
            let sc = if st >= 4 && rand(&mut self.rng) < 0.25 {
                hsv(&mut self.rng, 0.75, 1.0)
            } else {
                c
            };
            let vx = angle.cos() * speed;
            let vy = angle.sin() * speed - 70.0;
            let px = x + spread(&mut self.rng, w / 2.0);
            let py = y + spread(&mut self.rng, h / 2.0);
            self.parts.spawn(Particle {
                x: px,
                y: py,
                vx,
                vy,
                ay: 1000.0,
                life,
                max_life: life,
                size,
                color: sc,
                kind,
                drag: 0.5 + rand(&mut self.rng) * 0.6,
                spin: true,
            });
            // A short comet tail on some shards makes the motion readable.
            if i % 8 == 0 {
                for j in 1..=3 {
                    let t = j as f32;
                    self.parts.spawn(Particle {
                        x: px - vx * 0.018 * t,
                        y: py - vy * 0.018 * t,
                        vx: vx * 0.15,
                        vy: vy * 0.15,
                        ay: 0.0,
                        life: 0.16,
                        max_life: 0.16,
                        size: 2.4,
                        color: [sc[0], sc[1], sc[2], 0.6],
                        kind: SHAPE_GLOW,
                        drag: 2.0,
                        spin: false,
                    });
                }
            }
        }
        // Brief white-hot flash at the cell (stage 2+).
        if st >= 2 {
            self.parts.spawn(Particle {
                x: *x,
                y: *y,
                vx: 0.0,
                vy: 0.0,
                ay: 0.0,
                life: 0.06,
                max_life: 0.06,
                size: h.max(*w) * 2.4 * (1.0 + 0.6 * od),
                color: [1.0, 1.0, 1.0, 0.5],
                kind: SHAPE_DISC,
                drag: 0.0,
                spin: false,
            });
            // A white slash through the cell sells the "glass cut" moment.
            self.parts.spawn(Particle {
                x: *x,
                y: *y,
                vx: 1.0,
                vy: 1.0,
                ay: 0.0,
                life: 0.1,
                max_life: 0.1,
                size: h.max(*w) * 5.0,
                color: [1.0, 1.0, 1.0, 0.55],
                kind: SHAPE_BEAM,
                drag: 0.0,
                spin: true,
            });
        }
        // Shockwave ring from stage 2; stage 1 is just shards and sparks.
        if st >= 2 {
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
        }
        // Hot sparks unlock at stage 3.
        if st >= 3 {
            for _ in 0..(8.0 * density(scale)).round() as usize {
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
        // Stage 5 is deliberately excessive: a wide second shockwave, a light
        // pillar on the cell, and a ring of star satellites around the debris.
        if st >= 5 {
            self.parts.spawn(Particle {
                x: *x,
                y: *y,
                vx: 0.0,
                vy: 0.0,
                ay: 0.0,
                life: 0.42 * scale.max(0.5),
                max_life: 0.42 * scale.max(0.5),
                size: h.max(*w) * 2.6,
                color: [c[0], c[1], c[2], 0.35],
                kind: SHAPE_RING,
                drag: 0.0,
                spin: false,
            });
            self.parts.spawn(Particle {
                x: *x,
                y: *y,
                vx: 0.0,
                vy: 0.0,
                ay: 0.0,
                life: 0.3 * scale.max(0.5),
                max_life: 0.3 * scale.max(0.5),
                size: *h * 7.0,
                color: [c[0], c[1], c[2], 0.5],
                kind: SHAPE_BEAM,
                drag: 0.0,
                spin: false,
            });
            for i in 0..6 {
                let a = i as f32 * std::f32::consts::TAU / 6.0;
                let life = 0.35 + rand(&mut self.rng) * 0.2;
                self.parts.spawn(Particle {
                    x: *x,
                    y: *y,
                    vx: a.cos() * 190.0,
                    vy: a.sin() * 190.0 - 60.0,
                    ay: 350.0,
                    life,
                    max_life: life,
                    size: 3.0 + rand(&mut self.rng) * 1.5,
                    color: brighten(c, 1.2),
                    kind: SHAPE_STAR,
                    drag: 0.9,
                    spin: true,
                });
            }
        }
        // Variable reward: every 4th impact at max is a nova, so the same
        // update does not always pay out the same way (habituation).
        if nova {
            self.parts.spawn(Particle {
                x: *x,
                y: *y,
                vx: 0.0,
                vy: 0.0,
                ay: 0.0,
                life: 0.5 * scale.max(0.5),
                max_life: 0.5 * scale.max(0.5),
                size: h.max(*w) * 3.6,
                color: [1.0, 1.0, 1.0, 0.55],
                kind: SHAPE_RING,
                drag: 0.0,
                spin: false,
            });
            for i in 0..10 {
                let a = i as f32 * std::f32::consts::TAU / 10.0 + spread(&mut self.rng, 0.12);
                let life = 0.45 + rand(&mut self.rng) * 0.2;
                self.parts.spawn(Particle {
                    x: *x,
                    y: *y,
                    vx: a.cos() * 260.0,
                    vy: a.sin() * 260.0 - 80.0,
                    ay: 420.0,
                    life,
                    max_life: life,
                    size: 3.0 + rand(&mut self.rng) * 1.5,
                    color: brighten(c, 1.35),
                    kind: SHAPE_STAR,
                    drag: 0.8,
                    spin: true,
                });
            }
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

/// Saturated jewel tones for bullet-hell bursts: each shell pairs a color with
/// its opposite (index + 3) so overlapping rings stay readable.
const FIREWORK_COLORS: [[f32; 4]; 6] = [
    [1.0, 0.35, 0.35, 1.0],
    [0.4, 0.9, 1.0, 1.0],
    [1.0, 0.85, 0.3, 1.0],
    [0.75, 0.45, 1.0, 1.0],
    [0.45, 1.0, 0.55, 1.0],
    [1.0, 0.35, 0.85, 1.0],
];

/// One concentric bullet ring. Even angular spacing is what makes a burst read
/// as a bullet-hell "spell card" instead of random scatter.
struct Ring {
    n: usize,
    speed: f32,
    life: f32,
    size: f32,
    kind: u32,
    color: [f32; 4],
    /// Angular offset; stacked rings interleave instead of overlapping.
    phase: f32,
    /// Draw a smaller white-hot disc on top of every painted bullet.
    core: bool,
}

fn spawn_ring(parts: &mut Parts, rng: &mut u64, x: f32, y: f32, ring: &Ring) {
    if ring.n == 0 {
        return;
    }
    let step = std::f32::consts::TAU / ring.n as f32;
    for i in 0..ring.n {
        let a = ring.phase + i as f32 * step + spread(rng, step * 0.06);
        let v = ring.speed * (0.94 + rand(rng) * 0.12);
        let life = ring.life * (0.88 + rand(rng) * 0.24);
        let size = ring.size * (0.85 + rand(rng) * 0.3);
        let (vx, vy) = (a.cos() * v, a.sin() * v - v * 0.12);
        parts.spawn(Particle {
            x,
            y,
            vx,
            vy,
            ay: 90.0,
            life,
            max_life: life,
            size,
            color: ring.color,
            kind: ring.kind,
            drag: 0.45,
            spin: true,
        });
        if ring.core {
            parts.spawn(Particle {
                x,
                y,
                vx,
                vy,
                ay: 90.0,
                life: life * 0.7,
                max_life: life * 0.7,
                size: size * 0.42,
                color: [1.0, 1.0, 1.0, 0.95],
                kind: SHAPE_DISC,
                drag: 0.45,
                spin: false,
            });
        }
    }
}

fn palette_pair(rng: &mut u64) -> ([f32; 4], [f32; 4]) {
    let i = (rand(rng) * FIREWORK_COLORS.len() as f32) as usize % FIREWORK_COLORS.len();
    (
        FIREWORK_COLORS[i],
        FIREWORK_COLORS[(i + 3) % FIREWORK_COLORS.len()],
    )
}

/// Theme-park accent: four rainbow beams hugging the screen edges. Used at
/// 0.8+, so it never covers the text.
fn spawn_edge_pulse(parts: &mut Parts, rng: &mut u64, w: f32, h: f32, scale: f32) {
    let od = overdrive(scale);
    if od <= 0.0 {
        return;
    }
    let alpha = 0.2 + 0.25 * od;
    let life = 0.18 + 0.08 * od;
    let edges = [
        (w * 0.5, 4.0, w * 1.05, true),
        (w * 0.5, h - 4.0, w * 1.05, true),
        (4.0, h * 0.5, h * 1.05, false),
        (w - 4.0, h * 0.5, h * 1.05, false),
    ];
    for (x, y, size, horizontal) in edges {
        let color = hsv(rng, 0.75, 1.0);
        parts.spawn(Particle {
            x,
            y,
            vx: 0.0,
            vy: if horizontal { 1.0 } else { 0.0 },
            ay: 0.0,
            life,
            max_life: life,
            size,
            color: [color[0], color[1], color[2], alpha],
            kind: SHAPE_BEAM,
            drag: 0.0,
            spin: horizontal,
        });
    }
}

/// A Nyan-Cat-style ribbon: seven parallel rainbow beams across the screen.
fn spawn_rainbow_band(parts: &mut Parts, x: f32, y: f32, w: f32, scale: f32) {
    let od = overdrive(scale);
    if od <= 0.0 {
        return;
    }
    for i in 0..7 {
        let color = hsv_at(i as f32 / 7.0, 0.8, 1.0);
        let life = 0.22 + 0.02 * i as f32;
        parts.spawn(Particle {
            x,
            y: y + (i as f32 - 3.0) * 5.0,
            vx: 0.0,
            vy: 1.0,
            ay: 0.0,
            life,
            max_life: life,
            size: w * 1.1,
            color: [color[0], color[1], color[2], 0.14 + 0.14 * od],
            kind: SHAPE_BEAM,
            drag: 0.0,
            spin: true,
        });
    }
}

/// Command completions closer together than this are ignored: rapid commands
/// must not stack bursts into a flicker.
const FIREWORKS_COOLDOWN: f32 = 0.18;

/// Command-finished celebration: rockets rise from the cursor, then burst twice.
pub struct Fireworks {
    parts: Parts,
    rng: u64,
    /// Pending bursts: (fuse_seconds, x, y, color, accent, scale, jackpot).
    pending: Vec<(f32, f32, f32, [f32; 4], [f32; 4], f32, bool)>,
    /// Seconds until the next volley is accepted.
    cooldown: f32,
    /// Accepted volleys; every 3rd is a jackpot at 0.8+.
    bursts: u32,
}

impl Fireworks {
    pub fn new() -> Self {
        Self {
            parts: Parts::default(),
            rng: 0x27d4eb2f,
            pending: Vec::new(),
            cooldown: 0.0,
            bursts: 0,
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
        if self.cooldown > 0.0 {
            return;
        }
        self.cooldown = FIREWORKS_COOLDOWN;
        self.bursts = self.bursts.wrapping_add(1);
        let od = overdrive(scale);
        let st = stage(scale);
        let jackpot = od > 0.5 && self.bursts % 3 == 0;
        // One rocket per stage, then a final jump at the top of the dial.
        let mut n = match st {
            0 | 1 => 1,
            2 => 2,
            3 => 3,
            4 => 4,
            _ => 5,
        };
        if jackpot {
            n += 1;
        }
        for i in 0..n {
            // 0.8+ uses rainbow shells instead of the fixed palette.
            let (color, accent) = if od > 0.0 {
                (hsv(&mut self.rng, 0.7, 1.0), hsv(&mut self.rng, 0.7, 1.0))
            } else {
                palette_pair(&mut self.rng)
            };
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
            self.pending
                .push((fuse, tx, ty, color, accent, scale, jackpot));
        }
    }

    fn update(&mut self, dt: f32) {
        self.cooldown = (self.cooldown - dt).max(0.0);
        self.parts.step(dt);
        let mut bursts = Vec::new();
        self.pending.retain_mut(|p| {
            p.0 -= dt;
            if p.0 <= 0.0 {
                bursts.push((p.1, p.2, p.3, p.4, p.5, p.6));
                false
            } else {
                true
            }
        });
        for (bx, by, color, accent, scale, jackpot) in bursts {
            let od = overdrive(scale);
            let st = stage(scale);
            let dens = density(scale);
            let phase = rand(&mut self.rng) * std::f32::consts::TAU;
            // Outer shell: evenly spaced diamonds; cores unlock at stage 4.
            spawn_ring(
                &mut self.parts,
                &mut self.rng,
                bx,
                by,
                &Ring {
                    n: ((14.0 + 2.0 * st as f32) * dens).round().max(4.0) as usize,
                    speed: 220.0,
                    life: 0.9,
                    size: 5.2,
                    kind: SHAPE_DIAMOND,
                    color,
                    phase,
                    core: st >= 4,
                },
            );
            // Inner shell: stage 3 adds the slower opposite-tone stars.
            if st >= 3 {
                spawn_ring(
                    &mut self.parts,
                    &mut self.rng,
                    bx,
                    by,
                    &Ring {
                        n: ((8.0 + 2.0 * st as f32) * dens).round().max(2.0) as usize,
                        speed: 125.0,
                        life: 1.05,
                        size: 4.2,
                        kind: SHAPE_STAR,
                        color: with_alpha(accent, 0.78),
                        phase: std::f32::consts::PI / 16.0,
                        core: false,
                    },
                );
            }
            // Concussive flash at the burst point; stage 2 and up.
            if st >= 2 {
                self.parts.spawn(Particle {
                    x: bx,
                    y: by,
                    vx: 0.0,
                    vy: 0.0,
                    ay: 0.0,
                    life: 0.26,
                    max_life: 0.26,
                    size: 34.0 + 12.0 * od,
                    color: [1.0, 1.0, 1.0, 0.55 + 0.15 * od],
                    kind: SHAPE_RING,
                    drag: 0.0,
                    spin: false,
                });
            }
            // Glitter dust for depth; stage 2 and up.
            if st >= 2 {
                spawn_burst(
                    &mut self.parts,
                    &mut self.rng,
                    bx,
                    by,
                    ((6.0 + 2.0 * st as f32) * dens).round().max(2.0) as usize,
                    brighten(color, 1.25),
                    85.0,
                    0.55,
                );
            }
            // Secondary micro-burst arrives at stage 4.
            if st >= 4 {
                let mx = bx + spread(&mut self.rng, 45.0);
                let my = by + spread(&mut self.rng, 45.0);
                spawn_burst(
                    &mut self.parts,
                    &mut self.rng,
                    mx,
                    my,
                    ((10.0 + 3.0 * st as f32) * dens).round().max(2.0) as usize,
                    brighten(accent, 1.2),
                    95.0,
                    0.45,
                );
            }
            // Theme-park tier: every burst throws a dozen rainbow stars and a
            // short rainbow ribbon across the burst.
            if od > 0.0 {
                spawn_rainbow_band(&mut self.parts, bx, by, 700.0, scale);
                for _ in 0..12 {
                    let a = rand(&mut self.rng) * std::f32::consts::TAU;
                    let v = 120.0 + rand(&mut self.rng) * 220.0;
                    let life = 0.45 + rand(&mut self.rng) * 0.2;
                    self.parts.spawn(Particle {
                        x: bx,
                        y: by,
                        vx: a.cos() * v,
                        vy: a.sin() * v - 40.0,
                        ay: 260.0,
                        life,
                        max_life: life,
                        size: 2.6 + rand(&mut self.rng) * 1.6,
                        color: hsv(&mut self.rng, 0.75, 1.0),
                        kind: SHAPE_STAR,
                        drag: 0.9,
                        spin: true,
                    });
                }
            }
            // Jackpot burst: a white ring plus rainbow star satellites.
            if jackpot {
                self.parts.spawn(Particle {
                    x: bx,
                    y: by,
                    vx: 0.0,
                    vy: 0.0,
                    ay: 0.0,
                    life: 0.4,
                    max_life: 0.4,
                    size: 130.0,
                    color: [1.0, 1.0, 1.0, 0.35],
                    kind: SHAPE_RING,
                    drag: 0.0,
                    spin: false,
                });
                for i in 0..8 {
                    let a = i as f32 * std::f32::consts::TAU / 8.0;
                    let life = 0.4 + rand(&mut self.rng) * 0.2;
                    let v = 230.0 + rand(&mut self.rng) * 80.0;
                    self.parts.spawn(Particle {
                        x: bx,
                        y: by,
                        vx: a.cos() * v,
                        vy: a.sin() * v - 50.0,
                        ay: 300.0,
                        life,
                        max_life: life,
                        size: 3.2 + rand(&mut self.rng) * 1.5,
                        color: hsv(&mut self.rng, 0.75, 1.0),
                        kind: SHAPE_STAR,
                        drag: 0.8,
                        spin: true,
                    });
                }
            }
        }
    }

    fn collect(&self, out: &mut Vec<Instance>) {
        self.parts.collect(out);
    }
    fn active(&self) -> bool {
        self.parts.alive() || !self.pending.is_empty() || self.cooldown > 0.0
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

/// Enter bursts closer together than this are ignored: rapid repeats must not
/// stack into a flicker, and each burst needs a beat to read.
const CONFETTI_COOLDOWN: f32 = 0.12;

/// Enter: a bullet-hell "spell card" opening — impact core, interleaved star
/// and diamond rings, a delayed counter-wave, and paper confetti.
pub struct Confetti {
    parts: Parts,
    rng: u64,
    /// Pending follow-up waves: (delay_seconds, x, y, scale, color).
    pending: Vec<(f32, f32, f32, f32, [f32; 4])>,
    /// Seconds until the next burst is accepted.
    cooldown: f32,
    /// Accepted bursts; every 3rd is a jackpot at 0.8+.
    bursts: u32,
}

impl Confetti {
    pub fn new() -> Self {
        Self {
            parts: Parts::default(),
            rng: 0x51ed270b,
            pending: Vec::new(),
            cooldown: 0.0,
            bursts: 0,
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
        if self.cooldown > 0.0 {
            return;
        }
        self.cooldown = CONFETTI_COOLDOWN;
        self.bursts = self.bursts.wrapping_add(1);
        let od = overdrive(scale);
        let st = stage(scale);
        // 0.8+ turns the palette into a rainbow; every 3rd burst is a jackpot.
        let (color, accent) = if od > 0.0 {
            (hsv(&mut self.rng, 0.7, 1.0), hsv(&mut self.rng, 0.7, 1.0))
        } else {
            palette_pair(&mut self.rng)
        };
        let jackpot = od > 0.5 && self.bursts % 3 == 0;
        // Impact: a small white-hot point plus a crisp shockwave ring. A big
        // soft glow here just reads as a blurry circle over the text.
        self.parts.spawn(Particle {
            x: *x,
            y: *y,
            vx: 0.0,
            vy: 0.0,
            ay: 0.0,
            life: 0.12 * scale.max(0.5),
            max_life: 0.12 * scale.max(0.5),
            size: 26.0,
            color: [1.0, 1.0, 1.0, 0.95],
            kind: SHAPE_DISC,
            drag: 0.0,
            spin: false,
        });
        // Crisp shockwave rings instead of one thick donut; the wide one only
        // appears from stage 2 so low amounts stay local.
        let waves: &[(f32, f32, f32)] = if st >= 2 {
            &[(58.0, 0.95, 0.22), (128.0, 0.35, 0.3)]
        } else {
            &[(58.0, 0.95, 0.22)]
        };
        for (size, alpha, life) in waves {
            self.parts.spawn(Particle {
                x: *x,
                y: *y,
                vx: 0.0,
                vy: 0.0,
                ay: 0.0,
                life: life * scale.max(0.5),
                max_life: life * scale.max(0.5),
                size: *size,
                color: [color[0], color[1], color[2], *alpha],
                kind: SHAPE_RING,
                drag: 0.0,
                spin: false,
            });
        }
        // Crosshair beam: a thin vertical laser from the top edge through the
        // cursor, crossed by a fainter horizontal line on the cursor row.
        if st >= 4 {
            self.parts.spawn(Particle {
                x: *x,
                y: *y * 0.5,
                vx: 0.0,
                vy: 0.0,
                ay: 0.0,
                life: 0.22 * scale.max(0.5),
                max_life: 0.22 * scale.max(0.5),
                size: *y,
                color: [color[0], color[1], color[2], 0.85],
                kind: SHAPE_BEAM,
                drag: 0.0,
                spin: false,
            });
            self.parts.spawn(Particle {
                x: *w * 0.5,
                y: *y,
                vx: 0.0,
                // `Particle` derives rotation from velocity; a hair of downward
                // velocity turns the beam 90° without visibly moving it.
                vy: 1.0,
                ay: 0.0,
                life: 0.12 * scale.max(0.5),
                max_life: 0.12 * scale.max(0.5),
                size: *w * 0.6,
                color: [accent[0], accent[1], accent[2], 0.35],
                kind: SHAPE_BEAM,
                drag: 0.0,
                spin: true,
            });
        }
        // Primary ring: fast stars, white-hot cores from stage 4.
        let n = ((26.0 + 10.0 * od) * density(scale)).round().max(4.0) as usize;
        spawn_ring(
            &mut self.parts,
            &mut self.rng,
            *x,
            *y,
            &Ring {
                n,
                speed: 240.0,
                life: 0.7,
                size: 5.5,
                kind: SHAPE_STAR,
                color,
                phase: 0.0,
                core: st >= 4,
            },
        );
        // Counter ring: slower diamonds, half a step off so the gap is filled.
        if st >= 2 {
            spawn_ring(
                &mut self.parts,
                &mut self.rng,
                *x,
                *y,
                &Ring {
                    n: n / 2 + 5,
                    speed: 150.0,
                    life: 0.95,
                    size: 4.6,
                    kind: SHAPE_DIAMOND,
                    color: with_alpha(accent, 0.85),
                    phase: std::f32::consts::PI / n as f32,
                    core: false,
                },
            );
        }
        // Overdrive: a third, slower star shell between the two rings.
        if od > 0.0 {
            let m = (12.0 * od * density(scale)).round().max(1.0) as usize;
            spawn_ring(
                &mut self.parts,
                &mut self.rng,
                *x,
                *y,
                &Ring {
                    n: m,
                    speed: 200.0,
                    life: 0.6,
                    size: 4.2,
                    kind: SHAPE_STAR,
                    color: with_alpha(color, 0.9),
                    phase: std::f32::consts::PI / m as f32,
                    core: false,
                },
            );
        }
        // A second wave a beat later keeps the card from ending abruptly.
        if st >= 3 {
            self.pending.push((0.18, *x, *y, scale, accent));
        }
        if od > 0.5 {
            // Overdrive only: a later echo in the primary tone.
            self.pending.push((0.34, *x, *y, scale, color));
        }
        // A few petals around the cursor, not a curtain across the whole
        // screen: Enter is a routine action and must not bury the text.
        let n = ((16.0 + 8.0 * od) * density(scale)).round() as usize;
        let shapes = [SHAPE_RECT, SHAPE_STAR, SHAPE_DIAMOND, SHAPE_DIAMOND];
        for i in 0..n {
            let life = 0.6 + rand(&mut self.rng) * 0.5;
            let kind = shapes[i % shapes.len()];
            let px = (*x + spread(&mut self.rng, 170.0)).clamp(4.0, *w - 4.0);
            self.parts.spawn(Particle {
                x: px,
                y: *h + 8.0,
                vx: spread(&mut self.rng, 90.0),
                vy: -(520.0 + rand(&mut self.rng) * 380.0),
                ay: 900.0,
                life,
                max_life: life,
                size: 3.0 + rand(&mut self.rng) * 3.0,
                color: CONFETTI_COLORS[i % CONFETTI_COLORS.len()],
                kind,
                drag: 0.25,
                spin: true,
            });
        }
        // Theme-park tier: a rainbow ribbon, an edge frame, and free-floating
        // sparkles across the whole screen on every burst.
        if od > 0.0 {
            spawn_edge_pulse(&mut self.parts, &mut self.rng, *w, *h, scale);
            spawn_rainbow_band(&mut self.parts, *x, *y, *w, scale);
            for i in 0..10 {
                let a = i as f32 * std::f32::consts::TAU / 10.0;
                let color = hsv(&mut self.rng, 0.75, 1.0);
                let life = 0.4 + rand(&mut self.rng) * 0.2;
                let v = 150.0 + rand(&mut self.rng) * 90.0;
                self.parts.spawn(Particle {
                    x: *x,
                    y: *y,
                    vx: a.cos() * v,
                    vy: a.sin() * v - 40.0,
                    ay: 240.0,
                    life,
                    max_life: life,
                    size: 2.6 + rand(&mut self.rng) * 1.4,
                    color,
                    kind: SHAPE_STAR,
                    drag: 0.9,
                    spin: true,
                });
            }
            for _ in 0..20 {
                let life = 0.4 + rand(&mut self.rng) * 0.3;
                self.parts.spawn(Particle {
                    x: rand(&mut self.rng) * *w,
                    y: rand(&mut self.rng) * *h,
                    vx: spread(&mut self.rng, 30.0),
                    vy: 80.0 + rand(&mut self.rng) * 160.0,
                    ay: 120.0,
                    life,
                    max_life: life,
                    size: 1.5 + rand(&mut self.rng) * 1.5,
                    color: hsv(&mut self.rng, 0.8, 1.0),
                    kind: SHAPE_STAR,
                    drag: 0.4,
                    spin: true,
                });
            }
        }
        // Jackpot: a ring of rainbow stars on top of the usual burst.
        if jackpot {
            self.parts.spawn(Particle {
                x: *x,
                y: *y,
                vx: 0.0,
                vy: 0.0,
                ay: 0.0,
                life: 0.35,
                max_life: 0.35,
                size: w.max(*h) * 1.2,
                color: [1.0, 1.0, 1.0, 0.3],
                kind: SHAPE_RING,
                drag: 0.0,
                spin: false,
            });
            for i in 0..20 {
                let a = i as f32 * std::f32::consts::TAU / 20.0;
                let color = hsv(&mut self.rng, 0.75, 1.0);
                let life = 0.4 + rand(&mut self.rng) * 0.2;
                let v = 200.0 + rand(&mut self.rng) * 90.0;
                self.parts.spawn(Particle {
                    x: *x,
                    y: *y,
                    vx: a.cos() * v,
                    vy: a.sin() * v - 40.0,
                    ay: 260.0,
                    life,
                    max_life: life,
                    size: 3.0 + rand(&mut self.rng) * 1.5,
                    color,
                    kind: SHAPE_STAR,
                    drag: 0.8,
                    spin: true,
                });
            }
        }
    }

    fn update(&mut self, dt: f32) {
        self.cooldown = (self.cooldown - dt).max(0.0);
        self.parts.step(dt);
        let mut waves = Vec::new();
        self.pending.retain_mut(|p| {
            p.0 -= dt;
            if p.0 <= 0.0 {
                waves.push((p.1, p.2, p.3, p.4));
                false
            } else {
                true
            }
        });
        for (wx, wy, scale, color) in waves {
            let n = ((16.0 + 6.0 * overdrive(scale)) * density(scale))
                .round()
                .max(4.0) as usize;
            spawn_ring(
                &mut self.parts,
                &mut self.rng,
                wx,
                wy,
                &Ring {
                    n,
                    speed: 185.0,
                    life: 0.62,
                    size: 4.0,
                    kind: SHAPE_BOLT,
                    color: with_alpha(color, 0.75),
                    phase: std::f32::consts::PI / n as f32,
                    core: true,
                },
            );
        }
    }
    fn collect(&self, out: &mut Vec<Instance>) {
        self.parts.collect(out);
    }
    fn active(&self) -> bool {
        self.parts.alive() || !self.pending.is_empty() || self.cooldown > 0.0
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
        let len = (dx.hypot(*dy) * 0.015).clamp(4.0, 18.0) * density(scale);
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
        // Ghost rect at the departure cell; stage 2 and up.
        if stage(scale) >= 2 {
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

/// Scrollback motion: vertical speed lines in the direction the text moves.
/// Bounded per frame and in flight so fast wheel/trackpad scrolling reads as
/// motion instead of noise on top of the text.
pub struct ScrollTrail {
    parts: Parts,
    rng: u64,
}

/// Streaks alive at once during sustained scrolling.
const SCROLL_MAX_PARTS: usize = 240;
/// A single frame never spawns more than this many streaks, however far the
/// view jumped.
const SCROLL_MAX_STREAKS: usize = 12;

impl ScrollTrail {
    pub fn new() -> Self {
        Self {
            parts: Parts::default(),
            rng: 0x5eed_5c01,
        }
    }
}

impl Effect for ScrollTrail {
    fn name(&self) -> &str {
        "scroll_trail"
    }

    fn on_event(&mut self, ev: &FxEvent, scale: f32) {
        let FxEvent::Scrolled { lines, w, h } = ev else {
            return;
        };
        if *lines == 0 || self.parts.0.len() + 2 > SCROLL_MAX_PARTS {
            return;
        }
        let dir = if *lines > 0 { 1.0 } else { -1.0 };
        let lines = (*lines as f32).abs().min(30.0);
        let od = overdrive(scale);
        let n = ((2.0 + lines * 0.7) * density(scale) + 6.0 * od)
            .round()
            .clamp(1.0, SCROLL_MAX_STREAKS as f32) as usize;
        let len = (48.0 + lines * 3.0).min(110.0) * (0.7 + 0.3 * density(scale));
        for _ in 0..n {
            // One beam is a whole streak: no dotted tail and no vertical bias.
            if self.parts.0.len() + 2 > SCROLL_MAX_PARTS {
                return;
            }
            let x = rand(&mut self.rng) * *w;
            let y = rand(&mut self.rng) * *h;
            let speed = 240.0 + lines * 18.0;
            let color = if scale >= 0.8 {
                hsv(&mut self.rng, 0.6, 1.0)
            } else {
                [0.65, 0.9, 1.0, 1.0]
            };
            // Symmetric beam centered on y, moving with the content.
            self.parts.spawn(Particle {
                x,
                y,
                vx: spread(&mut self.rng, 1.0),
                vy: dir * speed,
                ay: 0.0,
                life: 0.2 + rand(&mut self.rng) * 0.1,
                max_life: 0.2 + rand(&mut self.rng) * 0.1,
                size: len,
                color: [color[0], color[1], color[2], 0.45],
                kind: SHAPE_BEAM,
                drag: 1.4,
                spin: false,
            });
            // A bright spark at the leading end gives the direction.
            let life = 0.12 + rand(&mut self.rng) * 0.06;
            self.parts.spawn(Particle {
                x: x + spread(&mut self.rng, 1.0),
                y: y + dir * len * 0.4,
                vx: spread(&mut self.rng, 4.0),
                vy: dir * speed * 1.15,
                ay: 0.0,
                life,
                max_life: life,
                size: 3.0 + rand(&mut self.rng) * 2.0,
                color: [color[0], color[1], color[2], 0.9],
                kind: SHAPE_GLOW,
                drag: 1.0,
                spin: false,
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
        let rings = if stage(scale) >= 2 { 2 } else { 1 };
        for i in 0..rings {
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
        for _ in 0..(6.0 * density(scale)).round() as usize {
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
        if stage(scale) >= 2 {
            for _ in 0..(5.0 * density(scale)).round() as usize {
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
    /// Top-tier storm factor (0 below 0.8, 1.0 at max).
    boost: f32,
    /// Sustained-output heat (0..1): rises with output, decays when quiet.
    heat: f32,
    /// Seconds until the next edge streak.
    edge_timer: f32,
}

impl OutputRain {
    pub fn new() -> Self {
        Self {
            parts: Parts::default(),
            rng: 0xdeadbabe,
            budget: 0.0,
            win_w: 960.0,
            win_h: 600.0,
            boost: 0.0,
            heat: 0.0,
            edge_timer: 0.0,
        }
    }
}

/// Shooting stars alive at once. Fast output keeps spawning, so the cap (not
/// the event rate) is what keeps a `cat` of a large file readable.
const RAIN_MAX_PARTS: usize = 220;
/// Stars spawned per frame; a burst must not become a wall of streaks.
const RAIN_MAX_SPAWNS: usize = 2;

/// Storm strength blends the top-tier factor with sustained-output heat, so
/// heavy output ramps up and then settles instead of staying flat.
fn storm(boost: f32, heat: f32) -> f32 {
    boost * (0.4 + 0.6 * heat)
}

fn rain_cap(boost: f32, heat: f32) -> usize {
    RAIN_MAX_PARTS + (RAIN_MAX_PARTS as f32 * 0.9 * storm(boost, heat)) as usize
}

fn rain_spawns(boost: f32, heat: f32) -> usize {
    RAIN_MAX_SPAWNS + (3.0 * storm(boost, heat)) as usize
}

impl Effect for OutputRain {
    fn name(&self) -> &str {
        "output_rain"
    }

    fn on_event(&mut self, ev: &FxEvent, scale: f32) {
        if let FxEvent::PtyOutput { n, w, h } = ev {
            self.boost = overdrive(scale);
            // Sustained output builds heat; short bursts barely move it.
            self.heat = (self.heat + (*n as f32).sqrt() * 0.004 * density(scale)).min(1.0);
            // Wakeups are coalesced per frame, but their count still grows
            // with throughput; sqrt keeps a big burst from multiplying.
            self.budget = (self.budget + (*n as f32).sqrt() * 0.10 * density(scale))
                .min(4.0 + 2.0 * self.boost);
            self.win_w = *w;
            self.win_h = *h;
        }
    }

    fn update(&mut self, dt: f32) {
        self.heat = (self.heat - dt * 0.8).max(0.0);
        self.edge_timer = (self.edge_timer - dt).max(0.0);
        self.parts.step(dt);
        let cap = if self.parts.0.len() >= rain_cap(self.boost, self.heat) {
            0
        } else {
            (self.budget as usize).min(rain_spawns(self.boost, self.heat))
        };
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
            let tail = 6 + (2.0 * self.boost) as usize;
            spawn_comet_tail(
                &mut self.parts,
                &mut self.rng,
                x,
                y,
                -speed,
                0.0,
                color,
                tail,
            );
        }
        // A hot storm at max also lights the top and bottom edges. Thin,
        // short-lived, and rate-limited: no full-screen flash.
        if self.boost > 0.5 && self.heat > 0.6 && self.edge_timer <= 0.0 {
            self.edge_timer = 0.22;
            let color = hsv(&mut self.rng, 0.55, 1.0);
            for y in [6.0, self.win_h - 6.0] {
                self.parts.spawn(Particle {
                    x: self.win_w * 0.5,
                    y,
                    vx: 0.0,
                    vy: 1.0,
                    ay: 0.0,
                    life: 0.18,
                    max_life: 0.18,
                    size: self.win_w * 1.1,
                    color: [color[0], color[1], color[2], 0.28],
                    kind: SHAPE_BEAM,
                    drag: 0.0,
                    spin: true,
                });
            }
        }
    }

    fn collect(&self, out: &mut Vec<Instance>) {
        self.parts.collect(out);
    }
    fn active(&self) -> bool {
        self.parts.alive() || self.budget >= 1.0 || self.heat > 0.0
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
        let mag = (1.0 + (*status as f32).abs() / 64.0).min(3.0) * density(scale);

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

#[cfg(test)]
mod tests {
    use super::*;

    fn output_event(n: usize) -> FxEvent {
        FxEvent::PtyOutput {
            n,
            w: 800.0,
            h: 600.0,
        }
    }

    /// Run `frames` ticks and return the peak live particle count.
    fn rain_peak(rain: &mut OutputRain, frames: usize) -> usize {
        let mut peak = 0;
        for _ in 0..frames {
            rain.update(1.0 / 60.0);
            peak = peak.max(rain.parts.0.len());
        }
        peak
    }

    #[test]
    fn output_rain_burst_stays_bounded() {
        // A huge read burst must not become thousands of shooting stars.
        let mut rain = OutputRain::new();
        rain.on_event(&output_event(10_000), 1.0);
        let peak = rain_peak(&mut rain, 180);
        assert!(
            peak <= rain_cap(1.0, 1.0),
            "max storm peaked at {peak} particles"
        );
    }

    #[test]
    fn output_rain_spawns_a_few_stars_per_frame() {
        let mut rain = OutputRain::new();
        rain.on_event(&output_event(10_000), 1.0);
        rain.update(1.0 / 60.0);
        // Each star is the head plus its boosted comet tail.
        let per_star = 1 + 6 + 2;
        assert!(rain.parts.0.len() <= rain_spawns(1.0, 1.0) * per_star);
    }

    #[test]
    fn output_rain_heats_up_and_settles() {
        let mut rain = OutputRain::new();
        rain.on_event(&output_event(10_000), 1.0);
        assert!(rain.heat > 0.4, "a big burst should build heat");
        for _ in 0..180 {
            rain.update(1.0 / 60.0);
        }
        assert_eq!(rain.heat, 0.0);
        assert!(!rain.active());
    }

    #[test]
    fn edge_streaks_only_appear_in_a_hot_max_storm() {
        let mut hot = OutputRain::new();
        hot.boost = 1.0;
        hot.heat = 1.0;
        hot.update(1.0 / 60.0);
        assert_eq!(
            hot.parts.0.iter().filter(|p| p.kind == SHAPE_BEAM).count(),
            2
        );
        let mut calm = OutputRain::new();
        calm.heat = 1.0;
        calm.update(1.0 / 60.0);
        assert!(!calm.parts.0.iter().any(|p| p.kind == SHAPE_BEAM));
    }

    #[test]
    fn output_rain_is_faint_at_the_bottom_of_the_dial() {
        let mut rain = OutputRain::new();
        rain.on_event(&output_event(10_000), 0.1);
        let peak = rain_peak(&mut rain, 30);
        assert!(peak <= 3, "0.1 should stay restrained, peaked at {peak}");
    }

    #[test]
    fn output_rain_reacts_to_ordinary_output() {
        let mut rain = OutputRain::new();
        rain.on_event(&output_event(100), 1.0);
        assert!(rain_peak(&mut rain, 5) > 0);
    }

    #[test]
    fn sub_star_budget_does_not_keep_redrawing() {
        let mut rain = OutputRain::new();
        rain.on_event(&output_event(1), 1.0);
        rain_peak(&mut rain, 120);
        assert!(!rain.active(), "fractional budget must not animate forever");
    }

    fn enter_event() -> FxEvent {
        FxEvent::Confetti {
            x: 400.0,
            y: 300.0,
            w: 800.0,
            h: 600.0,
        }
    }

    #[test]
    fn bullet_ring_is_evenly_spaced_with_hot_cores() {
        let mut parts = Parts::default();
        let mut rng = 7u64;
        spawn_ring(
            &mut parts,
            &mut rng,
            0.0,
            0.0,
            &Ring {
                n: 12,
                speed: 100.0,
                life: 1.0,
                size: 2.0,
                kind: SHAPE_STAR,
                color: [1.0; 4],
                phase: 0.0,
                core: true,
            },
        );
        assert_eq!(parts.0.len(), 24, "each bullet gets a white core");
        let sum: f32 = parts
            .0
            .iter()
            .filter(|p| p.kind == SHAPE_STAR)
            .map(|p| p.vx)
            .sum();
        assert!(sum.abs() < 30.0, "ring momentum should cancel, got {sum}");
    }

    #[test]
    fn confetti_fires_a_delayed_counter_wave() {
        let mut confetti = Confetti::new();
        confetti.on_event(&enter_event(), 1.0);
        confetti.update(0.019);
        assert!(
            !confetti.parts.0.iter().any(|p| p.kind == SHAPE_BOLT),
            "the follow-up wave must not fire immediately"
        );
        for _ in 0..20 {
            confetti.update(1.0 / 60.0);
        }
        assert!(
            confetti.parts.0.iter().any(|p| p.kind == SHAPE_BOLT),
            "the follow-up wave should fire a beat later"
        );
    }

    #[test]
    fn enter_burst_stays_bounded() {
        let mut confetti = Confetti::new();
        confetti.on_event(&enter_event(), 1.0);
        for _ in 0..120 {
            confetti.update(1.0 / 60.0);
            assert!(confetti.parts.0.len() < super::super::MAX_LIVE_PARTS);
        }
        assert!(!confetti.active(), "burst must settle");
    }

    #[test]
    fn enter_burst_uses_beams_not_a_giant_blur() {
        let mut confetti = Confetti::new();
        confetti.on_event(&enter_event(), 1.0);
        assert!(
            confetti.parts.0.iter().any(|p| p.kind == SHAPE_BEAM),
            "Enter should draw a crosshair beam"
        );
        assert!(
            !confetti
                .parts
                .0
                .iter()
                .any(|p| matches!(p.kind, SHAPE_GLOW | SHAPE_DISC) && p.size > 64.0),
            "the impact flash must stay a point, not a blurry disc"
        );
    }

    fn scroll_event(lines: i32) -> FxEvent {
        FxEvent::Scrolled {
            lines,
            w: 800.0,
            h: 600.0,
        }
    }

    #[test]
    fn scroll_trail_follows_the_scroll_direction() {
        let mut down = ScrollTrail::new();
        down.on_event(&scroll_event(3), 1.0);
        assert!(!down.parts.0.is_empty(), "scrolling must be visible");
        assert!(
            down.parts.0.iter().all(|p| p.vy > 0.0),
            "scrolling into history moves the text down"
        );
        let mut up = ScrollTrail::new();
        up.on_event(&scroll_event(-3), 1.0);
        assert!(up.parts.0.iter().all(|p| p.vy < 0.0));
    }

    #[test]
    fn scroll_trail_is_bounded_and_settles() {
        let mut trail = ScrollTrail::new();
        for _ in 0..120 {
            trail.on_event(&scroll_event(20), 1.0);
            trail.update(1.0 / 60.0);
            assert!(trail.parts.0.len() <= SCROLL_MAX_PARTS);
        }
        for _ in 0..120 {
            trail.update(1.0 / 60.0);
        }
        assert!(!trail.active(), "streaks must fade after scrolling stops");
    }

    fn key_event() -> FxEvent {
        FxEvent::Key {
            kind: KeyKind::Char,
            ch: Some('x'),
            x: 100.0,
            y: 100.0,
        }
    }

    fn command_done_event() -> FxEvent {
        FxEvent::CommandDone { x: 200.0, y: 200.0 }
    }

    fn erased_event() -> FxEvent {
        FxEvent::Erased {
            ch: 'x',
            x: 100.0,
            y: 100.0,
            w: 8.0,
            h: 18.0,
            color: [1.0, 0.5, 0.3, 1.0],
        }
    }

    #[test]
    fn every_fourth_impact_is_a_nova_at_max() {
        let mut normal = Shatter::new();
        normal.on_event(&erased_event(), 1.0);
        let base = normal.parts.0.len();

        let mut nova = Shatter::new();
        for _ in 0..3 {
            nova.on_event(&erased_event(), 1.0);
        }
        nova.parts.0.clear();
        nova.on_event(&erased_event(), 1.0);
        assert!(
            nova.parts.0.len() > base,
            "the 4th impact should pay out more"
        );

        // Below stage 5 the cadence stays flat.
        let mut calm_first = Shatter::new();
        calm_first.on_event(&erased_event(), 0.5);
        let mut calm = Shatter::new();
        for _ in 0..3 {
            calm.on_event(&erased_event(), 0.5);
        }
        calm.parts.0.clear();
        calm.on_event(&erased_event(), 0.5);
        assert_eq!(calm.parts.0.len(), calm_first.parts.0.len());
    }

    #[test]
    fn overdrive_only_unlocks_at_the_top() {
        assert_eq!(overdrive(0.0), 0.0);
        assert_eq!(overdrive(0.7), 0.0);
        assert_eq!(overdrive(0.8), 0.5);
        assert!((overdrive(0.9) - 0.75).abs() < 1e-6);
        assert!((overdrive(1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn stages_unlock_every_two_steps() {
        assert_eq!(stage(0.0), 0);
        assert_eq!(stage(0.1), 1);
        assert_eq!(stage(0.2), 1);
        assert_eq!(stage(0.3), 2);
        assert_eq!(stage(0.5), 3);
        assert_eq!(stage(0.7), 4);
        assert_eq!(stage(0.9), 5);
        assert_eq!(stage(1.0), 5);
    }

    #[test]
    fn density_rises_at_every_step() {
        let vals: Vec<f32> = (1..=10).map(|i| density(i as f32 / 10.0)).collect();
        for w in vals.windows(2) {
            assert!(
                w[1] - w[0] >= 0.08,
                "each 0.1 step should be visible: {vals:?}"
            );
        }
        assert!(density(1.0) > 1.3, "max should overshoot the old ceiling");
    }

    #[test]
    fn confetti_unlocks_layers_by_stage() {
        let mut low = Confetti::new();
        low.on_event(&enter_event(), 0.1);
        assert!(!low.parts.0.iter().any(|p| p.kind == SHAPE_DIAMOND));
        assert!(!low.parts.0.iter().any(|p| p.kind == SHAPE_BEAM));
        assert!(low.pending.is_empty());

        let mut mid = Confetti::new();
        mid.on_event(&enter_event(), 0.5);
        assert!(mid.parts.0.iter().any(|p| p.kind == SHAPE_DIAMOND));
        assert_eq!(mid.pending.len(), 1);
        assert!(!mid.parts.0.iter().any(|p| p.kind == SHAPE_BEAM));

        let mut high = Confetti::new();
        high.on_event(&enter_event(), 1.0);
        assert!(high.parts.0.iter().any(|p| p.kind == SHAPE_BEAM));
        assert_eq!(high.pending.len(), 2);
    }

    #[test]
    fn fireworks_add_a_rocket_per_stage() {
        let mut low = Fireworks::new();
        low.on_event(&command_done_event(), 0.1);
        assert_eq!(low.pending.len(), 1);
        let mut mid = Fireworks::new();
        mid.on_event(&command_done_event(), 0.5);
        assert_eq!(mid.pending.len(), 3);
        let mut high = Fireworks::new();
        high.on_event(&command_done_event(), 1.0);
        assert_eq!(high.pending.len(), 5);
    }

    #[test]
    fn combo_tracks_fast_typing_and_resets_when_idle() {
        let mut sparks = Sparks::new();
        sparks.on_event(&key_event(), 1.0);
        assert_eq!(sparks.combo, 1);
        sparks.update(0.2);
        sparks.on_event(&key_event(), 1.0);
        assert_eq!(sparks.combo, 2);
        sparks.update(COMBO_RESET + 0.01);
        assert_eq!(sparks.combo, 0);
    }

    #[test]
    fn overdrive_typing_earns_milestone_stars() {
        let mut hot = Sparks::new();
        for _ in 0..10 {
            hot.on_event(&key_event(), 1.0);
            hot.update(0.1);
        }
        assert!(
            hot.parts.0.iter().any(|p| p.kind == SHAPE_STAR),
            "max amount should celebrate every 10 keys"
        );
        let mut calm = Sparks::new();
        for _ in 0..10 {
            calm.on_event(&key_event(), 0.4);
            calm.update(0.1);
        }
        assert!(
            !calm.parts.0.iter().any(|p| p.kind == SHAPE_STAR),
            "lower amounts only celebrate at 25/50"
        );
    }

    #[test]
    fn rapid_enter_bursts_wait_for_the_cooldown() {
        let mut confetti = Confetti::new();
        confetti.on_event(&enter_event(), 1.0);
        let first = confetti.parts.0.len();
        confetti.on_event(&enter_event(), 1.0);
        assert_eq!(confetti.parts.0.len(), first, "second burst must wait");
        confetti.update(CONFETTI_COOLDOWN + 0.01);
        let before = confetti.parts.0.len();
        confetti.on_event(&enter_event(), 1.0);
        assert!(confetti.parts.0.len() > before);
    }

    #[test]
    fn rapid_command_completions_wait_for_the_cooldown() {
        let mut fireworks = Fireworks::new();
        fireworks.on_event(&command_done_event(), 1.0);
        let first = fireworks.pending.len();
        assert!(first > 0);
        fireworks.on_event(&command_done_event(), 1.0);
        assert_eq!(fireworks.pending.len(), first, "second volley must wait");
        fireworks.update(FIREWORKS_COOLDOWN + 0.01);
        fireworks.on_event(&command_done_event(), 1.0);
        assert_eq!(fireworks.pending.len(), first * 2);
    }

    #[test]
    fn max_amount_adds_layers_without_covering_more_screen() {
        let mut hot = Confetti::new();
        hot.on_event(&enter_event(), 1.0);
        let mut calm = Confetti::new();
        calm.on_event(&enter_event(), 0.8);
        let stars = |c: &Confetti| c.parts.0.iter().filter(|p| p.kind == SHAPE_STAR).count();
        assert!(
            stars(&hot) > stars(&calm),
            "overdrive must add bullet layers, not just brighten"
        );
    }
}
