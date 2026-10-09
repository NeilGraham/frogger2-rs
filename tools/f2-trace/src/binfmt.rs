//! Compact binary encoding of a trace (about 5-8x smaller than JSON lines).
//!
//! Layout: [`MAGIC`] (8 bytes), schema version (u32 LE), then per frame a
//! LEB128 byte length and the frame's fields in declaration order.
//! Unsigned integers are LEB128, signed ones zigzag + LEB128; `Option` is a
//! tag byte (0/1) followed by the value; `Vec` is a LEB128 count followed by
//! the elements; fixed arrays have no count.

use crate::{EnemyState, GaribState, PlatformState, PlayerState, TraceFrame, SCHEMA_VERSION};

pub const MAGIC: &[u8; 8] = b"F2TRACE\0";

#[derive(Default)]
struct W(Vec<u8>);

impl W {
    fn u(&mut self, mut v: u64) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                self.0.push(b);
                return;
            }
            self.0.push(b | 0x80);
        }
    }
    fn i(&mut self, v: i64) {
        self.u(((v << 1) ^ (v >> 63)) as u64);
    }
    fn pos(&mut self, p: &Option<[i16; 3]>) {
        match p {
            None => self.0.push(0),
            Some(p) => {
                self.0.push(1);
                p.iter().for_each(|&c| self.i(c as i64));
            }
        }
    }
}

struct R<'a> {
    b: &'a [u8],
    at: usize,
}

impl R<'_> {
    fn byte(&mut self) -> Result<u8, String> {
        let v = *self.b.get(self.at).ok_or("truncated")?;
        self.at += 1;
        Ok(v)
    }
    fn u(&mut self) -> Result<u64, String> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let b = self.byte()?;
            v |= ((b & 0x7f) as u64) << shift;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err("varint too long".into())
    }
    fn i(&mut self) -> Result<i64, String> {
        let u = self.u()?;
        Ok((u >> 1) as i64 ^ -((u & 1) as i64))
    }
    fn u32(&mut self) -> Result<u32, String> {
        u32::try_from(self.u()?).map_err(|e| e.to_string())
    }
    fn u16(&mut self) -> Result<u16, String> {
        u16::try_from(self.u()?).map_err(|e| e.to_string())
    }
    fn u8(&mut self) -> Result<u8, String> {
        u8::try_from(self.u()?).map_err(|e| e.to_string())
    }
    fn i32(&mut self) -> Result<i32, String> {
        i32::try_from(self.i()?).map_err(|e| e.to_string())
    }
    fn i16(&mut self) -> Result<i16, String> {
        i16::try_from(self.i()?).map_err(|e| e.to_string())
    }
    fn i8(&mut self) -> Result<i8, String> {
        i8::try_from(self.i()?).map_err(|e| e.to_string())
    }
    fn len(&mut self) -> Result<usize, String> {
        let n = self.u()? as usize;
        if n > self.b.len() - self.at {
            return Err(format!("count {n} exceeds the remaining data"));
        }
        Ok(n)
    }
    fn pos(&mut self) -> Result<Option<[i16; 3]>, String> {
        match self.byte()? {
            0 => Ok(None),
            1 => Ok(Some([self.i16()?, self.i16()?, self.i16()?])),
            t => Err(format!("bad option tag {t}")),
        }
    }
}

fn put_frame(w: &mut W, f: &TraceFrame) {
    w.u(f.v as u64);
    w.u(f.seq as u64);
    w.u(f.act_frame_count as u64);
    w.i(f.frame_count as i64);
    w.i(f.game_speed as i64);
    w.u(f.mode as u64);
    for b in [f.multi, f.single, f.world, f.level] {
        w.u(b as u64);
    }
    w.u(f.rseed as u64);
    f.pads.iter().for_each(|&p| w.u(p as u64));
    w.u(f.players.len() as u64);
    for p in &f.players {
        w.pos(&p.pos);
        w.u(p.frog_state as u64);
        w.i(p.curr_tile as i64);
        w.i(p.lives as i64);
        for b in [p.health, p.death_by, p.can_jump, p.has_jumped, p.is_super_hopping, p.is_sinking] {
            w.u(b as u64);
        }
        w.i(p.num_spawn as i64);
        w.u(p.safe_time as u64);
        w.u(p.dead_time as u64);
        w.i(p.jump_time as i64);
        w.i(p.height_jumped as i64);
    }
    w.u(f.enemies.len() as u64);
    for e in &f.enemies {
        w.i(e.uid as i64);
        w.u(e.active as u64);
        w.pos(&e.pos);
        w.u(e.flags as u64);
        w.i(e.is_waiting as i64);
        w.i(e.speed as i64);
        w.i(e.in_tile as i64);
    }
    w.u(f.platforms.len() as u64);
    for p in &f.platforms {
        w.i(p.uid as i64);
        w.u(p.active as u64);
        w.pos(&p.pos);
        w.u(p.flags as u64);
        w.i(p.is_waiting as i64);
        w.i(p.curr_speed as i64);
        p.in_tile.iter().for_each(|&t| w.i(t as i64));
    }
    w.i(f.garib_count as i64);
    w.i(f.garib_total as i64);
    w.u(f.garibs.len() as u64);
    for g in &f.garibs {
        w.u(g.kind as u64);
        w.i(g.active as i64);
        g.pos.iter().for_each(|&c| w.i(c as i64));
    }
    w.i(f.babies_saved as i64);
    w.i(f.num_babies as i64);
    w.u(f.end_level_timer as u64);
    f.cam_source.iter().chain(&f.cam_target).for_each(|&c| w.i(c as i64));
}

fn get_frame(r: &mut R) -> Result<TraceFrame, String> {
    let mut f = TraceFrame {
        v: r.u32()?,
        seq: r.u32()?,
        act_frame_count: r.u32()?,
        frame_count: r.i32()?,
        game_speed: r.i32()?,
        mode: r.u32()?,
        multi: r.u8()?,
        single: r.u8()?,
        world: r.u8()?,
        level: r.u8()?,
        rseed: r.u32()?,
        ..Default::default()
    };
    for p in f.pads.iter_mut() {
        *p = r.u16()?;
    }
    for _ in 0..r.len()? {
        f.players.push(PlayerState {
            pos: r.pos()?,
            frog_state: r.u32()?,
            curr_tile: r.i32()?,
            lives: r.i16()?,
            health: r.u8()?,
            death_by: r.u8()?,
            can_jump: r.u8()?,
            has_jumped: r.u8()?,
            is_super_hopping: r.u8()?,
            is_sinking: r.u8()?,
            num_spawn: r.i16()?,
            safe_time: r.u16()?,
            dead_time: r.u16()?,
            jump_time: r.i32()?,
            height_jumped: r.i32()?,
        });
    }
    for _ in 0..r.len()? {
        f.enemies.push(EnemyState { uid: r.i32()?, active: r.u8()?, pos: r.pos()?, flags: r.u32()?, is_waiting: r.i16()?, speed: r.i32()?, in_tile: r.i32()? });
    }
    for _ in 0..r.len()? {
        f.platforms.push(PlatformState {
            uid: r.i32()?,
            active: r.u8()?,
            pos: r.pos()?,
            flags: r.u32()?,
            is_waiting: r.i16()?,
            curr_speed: r.i32()?,
            in_tile: [r.i32()?, r.i32()?],
        });
    }
    f.garib_count = r.i16()?;
    f.garib_total = r.i16()?;
    for _ in 0..r.len()? {
        f.garibs.push(GaribState { kind: r.u8()?, active: r.i8()?, pos: [r.i16()?, r.i16()?, r.i16()?] });
    }
    f.babies_saved = r.i32()?;
    f.num_babies = r.i32()?;
    f.end_level_timer = r.u16()?;
    for c in f.cam_source.iter_mut().chain(f.cam_target.iter_mut()) {
        *c = r.i32()?;
    }
    Ok(f)
}

/// Encode a whole trace.
pub fn encode(frames: &[TraceFrame]) -> Vec<u8> {
    let mut out = W(MAGIC.to_vec());
    out.0.extend_from_slice(&SCHEMA_VERSION.to_le_bytes());
    for f in frames {
        let mut w = W::default();
        put_frame(&mut w, f);
        out.u(w.0.len() as u64);
        out.0.extend_from_slice(&w.0);
    }
    out.0
}

/// Decode a whole trace.
pub fn decode(bytes: &[u8]) -> Result<Vec<TraceFrame>, String> {
    if !bytes.starts_with(MAGIC) || bytes.len() < 12 {
        return Err("not an f2-trace binary file".into());
    }
    let ver = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    if ver != SCHEMA_VERSION {
        return Err(format!("schema version {ver}, expected {SCHEMA_VERSION}"));
    }
    let mut r = R { b: bytes, at: 12 };
    let mut out = Vec::new();
    while r.at < bytes.len() {
        let n = r.len()?;
        let end = r.at + n;
        let mut fr = R { b: &bytes[..end], at: r.at };
        let f = get_frame(&mut fr).map_err(|e| format!("frame {}: {e}", out.len()))?;
        if fr.at != end {
            return Err(format!("frame {}: {} trailing bytes", out.len(), end - fr.at));
        }
        out.push(f);
        r.at = end;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample;

    #[test]
    fn round_trip_and_size() {
        let frames: Vec<_> = (0..50).map(sample).collect();
        let b = encode(&frames);
        assert_eq!(decode(&b).unwrap(), frames);
        let json: usize = frames.iter().map(|f| f.to_json_line().len() + 1).sum();
        assert!(b.len() * 4 < json, "binary {} vs json {}", b.len(), json);
    }

    #[test]
    fn extremes_and_errors() {
        let mut f = sample(0);
        f.rseed = u32::MAX;
        f.cam_target = [i32::MIN, i32::MAX, -1];
        f.players[0].pos = Some([i16::MIN, i16::MAX, 0]);
        let b = encode(std::slice::from_ref(&f));
        assert_eq!(decode(&b).unwrap(), vec![f]);
        assert!(decode(&b[..b.len() - 1]).is_err());
        assert!(decode(b"F2TRACE\0\x02\0\0\0").is_err());
        assert!(decode(b"{}").is_err());
    }
}
