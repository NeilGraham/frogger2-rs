//! Drawing the simulation state: the shared draw loop lives in `f2-draw` (re-exported
//! here); this module adds the desktop's debug free camera.

pub use f2_draw::render::*;
use f2_engine::Camera;
use f2_platform::dik;
use f2_platform::KeyboardState;

/// Free-flying debug camera (F1).
#[derive(Clone, Copy, Debug)]
pub struct FlyCam {
    pub pos: V3,
    pub yaw: f32,
    pub pitch: f32,
}

impl FlyCam {
    pub fn from_camera(c: &Camera) -> FlyCam {
        let d = norm(sub(c.target, c.source));
        FlyCam { pos: c.source, yaw: d[0].atan2(d[2]), pitch: d[1].clamp(-1.0, 1.0).asin() }
    }
    fn forward(&self) -> V3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        [cp * sy, sp, cp * cy]
    }
    pub fn camera(&self) -> Camera {
        Camera { source: self.pos, target: add(self.pos, mul(self.forward(), 100.0)), up: [0.0, 1.0, 0.0] }
    }
    /// WASD move, Q/E down/up, arrow keys look, Shift fast.
    pub fn update(&mut self, kb: &KeyboardState, dt: f32) {
        let turn = 1.8 * dt;
        if kb.is_down(dik::LEFT) {
            self.yaw += turn;
        }
        if kb.is_down(dik::RIGHT) {
            self.yaw -= turn;
        }
        if kb.is_down(dik::UP) {
            self.pitch = (self.pitch + turn).min(1.55);
        }
        if kb.is_down(dik::DOWN) {
            self.pitch = (self.pitch - turn).max(-1.55);
        }
        let f = self.forward();
        let right = norm(cross(f, [0.0, 1.0, 0.0]));
        let mut mv = [0.0; 3];
        for (k, d) in [(dik::W, f), (dik::S, mul(f, -1.0)), (dik::D, right), (dik::A, mul(right, -1.0)), (dik::E, [0.0, 1.0, 0.0]), (dik::Q, [0.0, -1.0, 0.0])]
        {
            if kb.is_down(k) {
                mv = add(mv, d);
            }
        }
        let speed = if kb.is_down(dik::LSHIFT) || kb.is_down(dik::RSHIFT) { 1200.0 } else { 300.0 };
        self.pos = add(self.pos, mul(mv, speed * dt));
    }
}
