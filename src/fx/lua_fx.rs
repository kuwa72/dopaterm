//! Lua-scripted effects.
//!
//! A `.lua` file in the effects dir becomes an Effect. Contract:
//!
//! ```lua
//! -- optional: fx_name = "name"
//! function on_event(e)           -- e.type, e.x, e.y, e.w, e.h, e.ch, e.kind, e.dx, e.dy
//!   fx.spawn{ x=e.x, y=e.y, vx=0, vy=-100, ay=300,
//!             life=0.4, size=3, r=1, g=0.5, b=1, a=1, kind=1 }
//! end
//! function update(dt) end        -- optional; advance custom state
//! ```
//!
//! `fx.spawn` queues a particle for this tick; `fx.parts` is managed
//! internally (spawned particles auto-step with physics each frame).

use super::{Effect, FxEvent, Instance, Particle, Parts};
use crate::input::KeyKind;
use mlua::{Function, Lua, Table};

pub struct LuaEffect {
    lua: Lua,
    name: String,
    parts: Parts,
}

impl LuaEffect {
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        let lua = Lua::new();
        let src = std::fs::read_to_string(path)?;

        // fx.spawn{...} -> push into a registry queue (drained after each callback).
        let queue = lua.create_table()?;
        lua.set_named_registry_value("dopa_fx_queue", queue)?;
        let fx = lua.create_table()?;
        let queue = lua.named_registry_value::<Table>("dopa_fx_queue")?;
        fx.set(
            "spawn",
            lua.create_function(move |_, t: Table| {
                queue.set(queue.raw_len() + 1, t)?;
                Ok(())
            })?,
        )?;
        lua.globals().set("fx", fx)?;

        lua.load(&src).set_name(path.to_string_lossy()).exec()?;

        let name = lua
            .globals()
            .get::<Option<String>>("fx_name")?
            .unwrap_or_else(|| {
                path.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            });
        Ok(Self {
            lua,
            name,
            parts: Parts::default(),
        })
    }

    fn drain_queue(&mut self, scale: f32) {
        let Ok(queue) = self.lua.named_registry_value::<Table>("dopa_fx_queue") else {
            return;
        };
        for t in queue.sequence_values::<Table>().flatten() {
            let g = |k: &str| t.get::<Option<f32>>(k).ok().flatten().unwrap_or(0.0);
            let life = g("life").max(0.01);
            self.parts.spawn(Particle {
                x: g("x"),
                y: g("y"),
                vx: g("vx"),
                vy: g("vy"),
                ay: g("ay"),
                life: life * scale.max(0.05),
                max_life: life * scale.max(0.05),
                size: g("size").max(1.0),
                color: [
                    g("r"),
                    g("g"),
                    g("b"),
                    if g("a") == 0.0 { 1.0 } else { g("a") },
                ],
                kind: g("kind") as u32,
                drag: g("drag"),
                spin: g("spin") != 0.0,
            });
        }
        queue.clear().ok();
    }
}

impl Effect for LuaEffect {
    fn name(&self) -> &str {
        &self.name
    }

    fn on_event(&mut self, ev: &FxEvent, scale: f32) {
        let g = self.lua.globals();
        let Ok(f) = g.get::<Option<Function>>("on_event") else {
            return;
        };
        let Some(f) = f else { return };
        if let Ok(e) = self.event_table(ev) {
            let _ = f.call::<()>(e);
        }
        self.drain_queue(scale);
    }

    fn update(&mut self, dt: f32) {
        let g = self.lua.globals();
        if let Ok(Some(f)) = g.get::<Option<Function>>("update") {
            let _ = f.call::<()>(dt);
        }
        self.drain_queue(1.0);
        self.parts.step(dt);
    }

    fn collect(&self, out: &mut Vec<Instance>) {
        self.parts.collect(out);
    }
    fn active(&self) -> bool {
        self.parts.alive()
    }
}

impl LuaEffect {
    fn event_table(&self, ev: &FxEvent) -> mlua::Result<Table> {
        let t = self.lua.create_table()?;
        match ev {
            FxEvent::Key { kind, ch, x, y } => {
                t.set("type", "key")?;
                t.set(
                    "kind",
                    match kind {
                        KeyKind::Char => "char",
                        KeyKind::Backspace => "backspace",
                        KeyKind::Enter => "enter",
                        KeyKind::Arrow => "arrow",
                        KeyKind::Other => "other",
                    },
                )?;
                t.set("ch", ch.map(|c| c.to_string()).unwrap_or_default())?;
                t.set("x", *x)?;
                t.set("y", *y)?;
            }
            FxEvent::Erased {
                ch,
                x,
                y,
                w,
                h,
                color,
            } => {
                t.set("type", "erased")?;
                t.set("ch", ch.to_string())?;
                t.set("x", *x)?;
                t.set("y", *y)?;
                t.set("w", *w)?;
                t.set("h", *h)?;
                t.set("r", color[0])?;
                t.set("g", color[1])?;
                t.set("b", color[2])?;
            }
            FxEvent::CursorMoved { x, y, w, h, dx, dy } => {
                t.set("type", "cursor_moved")?;
                t.set("x", *x)?;
                t.set("y", *y)?;
                t.set("w", *w)?;
                t.set("h", *h)?;
                t.set("dx", *dx)?;
                t.set("dy", *dy)?;
            }
            FxEvent::PtyOutput { n, w, h } => {
                t.set("type", "pty_output")?;
                t.set("n", *n)?;
                t.set("w", *w)?;
                t.set("h", *h)?;
            }
            FxEvent::Bell { x, y, w, h } => {
                t.set("type", "bell")?;
                t.set("x", *x)?;
                t.set("y", *y)?;
                t.set("w", *w)?;
                t.set("h", *h)?;
            }
            FxEvent::CommandDone { x, y } => {
                t.set("type", "command_done")?;
                t.set("x", *x)?;
                t.set("y", *y)?;
            }
            FxEvent::Waiting { x, y } => {
                t.set("type", "waiting")?;
                t.set("x", *x)?;
                t.set("y", *y)?;
            }
            FxEvent::Confetti { x, y, w, h } => {
                t.set("type", "confetti")?;
                t.set("x", *x)?;
                t.set("y", *y)?;
                t.set("w", *w)?;
                t.set("h", *h)?;
            }
            FxEvent::ChildError { x, y, w, h, status } => {
                t.set("type", "child_error")?;
                t.set("x", *x)?;
                t.set("y", *y)?;
                t.set("w", *w)?;
                t.set("h", *h)?;
                t.set("status", *status)?;
            }
        }
        Ok(t)
    }
}
