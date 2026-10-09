//! Effect plugin layer.
//!
//! Model: terminal events (`FxEvent`) -> effects emit `Particle`s -> renderer
//! draws them as instanced quads over the cell grid. Effects never block input
//! and never mutate terminal state (dopairb design principle).

pub mod builtin;
pub mod lua_fx;

use crate::input::KeyKind;

/// Visual shape encoded in `Instance.kind` and `Particle.kind`.
pub const SHAPE_RECT: u32 = 0;
pub const SHAPE_GLOW: u32 = 1;
pub const SHAPE_RING: u32 = 2;
pub const SHAPE_DIAMOND: u32 = 3;
pub const SHAPE_STAR: u32 = 4;
pub const SHAPE_BOLT: u32 = 5;
pub const SHAPE_DISC: u32 = 6;

/// GPU instance data for one quad. Pixel-space position.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Instance {
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub rot: f32,
    /// Shape id: rect, glow, ring, diamond, star, bolt, disc.
    pub kind: u32,
    pub color: [f32; 4],
}

/// A particle owned by an effect. Positions are in pixels.
#[derive(Clone, Copy, Debug)]
pub struct Particle {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    pub ay: f32,
    pub life: f32,
    pub max_life: f32,
    pub size: f32,
    pub color: [f32; 4],
    pub kind: u32,
    pub drag: f32,
    pub spin: bool,
}

impl Particle {
    fn step(&mut self, dt: f32) {
        self.life -= dt;
        self.vx *= 1.0 - self.drag * dt;
        self.vy *= 1.0 - self.drag * dt;
        self.vy += self.ay * dt;
        self.x += self.vx * dt;
        self.y += self.vy * dt;
    }

    fn instance(&self) -> Instance {
        let t = (self.life / self.max_life).clamp(0.0, 1.0);
        let mut c = self.color;
        c[3] *= t;
        let rot = if self.spin {
            self.vy.atan2(self.vx)
        } else {
            0.0
        };
        Instance {
            pos: [self.x, self.y],
            size: [self.size, self.size],
            rot,
            kind: self.kind,
            color: c,
        }
    }
}

/// Terminal events effects can react to. Coordinates are pixel-space cell
/// centers/rects already resolved by the app layer.
#[derive(Clone, Debug)]
pub enum FxEvent {
    /// A printable key (or other key) was sent to the PTY. `x,y` is the cursor
    /// cell center at the moment of input. `ch` is the first typed character, if any.
    Key {
        kind: KeyKind,
        ch: Option<char>,
        x: f32,
        y: f32,
    },
    /// A cell that previously held `ch` was erased (cleared or overwritten by
    /// space). `x,y,w,h` is the cell rect.
    Erased {
        ch: char,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: [f32; 4],
    },
    /// Cursor jumped. `dx,dy` is pixel delta direction.
    CursorMoved {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        dx: f32,
        dy: f32,
    },
    /// `n` units of PTY output were consumed this frame; `w,h` is the window
    /// size in pixels so effects can rain across the full screen.
    PtyOutput { n: usize, w: f32, h: f32 },
    /// Terminal bell.
    Bell { x: f32, y: f32, w: f32, h: f32 },
    /// Heuristic: Enter was followed by PTY output which then went quiet.
    /// `x,y` is the cursor cell center.
    CommandDone { x: f32, y: f32 },
    /// Heuristic: no input/output for a while; emitted periodically while the
    /// terminal appears to be waiting for input. `x,y` is the cursor center.
    Waiting { x: f32, y: f32 },
    /// Enter was pressed. `x,y` is the cursor center; `w,h` is the window size
    /// in pixels so effects can span the full screen.
    Confetti { x: f32, y: f32, w: f32, h: f32 },
    /// Child/grandchild process exited with a non-zero status. `x,y` is the
    /// window center; `w,h` is the window size; `status` is the exit code.
    ChildError {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        status: i32,
    },
}

/// Effect plugin interface. `scale` is the global intensity multiplier.
pub trait Effect {
    fn name(&self) -> &str;
    fn on_event(&mut self, _ev: &FxEvent, _scale: f32) {}
    fn update(&mut self, _dt: f32) {}
    fn collect(&self, _out: &mut Vec<Instance>) {}
    fn active(&self) -> bool {
        false
    }
}

/// Particle container usable by any Effect impl.
#[derive(Default)]
pub struct Parts(pub Vec<Particle>);

impl Parts {
    pub fn spawn(&mut self, p: Particle) {
        self.0.push(p);
    }
    pub fn step(&mut self, dt: f32) {
        self.0.retain_mut(|p| {
            p.step(dt);
            p.life > 0.0
        });
    }
    pub fn collect(&self, out: &mut Vec<Instance>) {
        out.extend(self.0.iter().map(Particle::instance));
    }
    pub fn alive(&self) -> bool {
        !self.0.is_empty()
    }
}

pub struct Manager {
    effects: Vec<Box<dyn Effect>>,
    scale: f32,
    scratch: Vec<Instance>,
}

impl Manager {
    pub fn new(scale: f32) -> Self {
        Self {
            effects: Vec::new(),
            scale,
            scratch: Vec::new(),
        }
    }

    pub fn push(&mut self, e: Box<dyn Effect>) {
        self.effects.push(e);
    }

    pub fn clear(&mut self) {
        self.effects.clear();
    }

    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale;
    }

    pub fn event(&mut self, ev: &FxEvent) {
        if self.scale <= 0.0 {
            return;
        }
        for e in &mut self.effects {
            e.on_event(ev, self.scale);
        }
    }

    /// Advance all effects; returns true while animation continues.
    pub fn tick(&mut self, dt: f32) -> bool {
        let mut any = false;
        for e in &mut self.effects {
            e.update(dt);
            any |= e.active();
        }
        any
    }

    pub fn instances(&mut self) -> &[Instance] {
        self.scratch.clear();
        for e in &self.effects {
            e.collect(&mut self.scratch);
        }
        &self.scratch
    }
}
