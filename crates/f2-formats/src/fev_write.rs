//! Compat wrappers and the byte-exact writer for `.fev` scripts (child module of `fev`).

use super::*;

/// Reads one trigger (raw operand values are discarded; use [`Script::parse`] to keep them).
pub fn read_trigger(r: &mut Reader, depth: u32) -> Result<Trigger> {
    trigger_ctx(r, depth, &mut Vec::new())
}

/// Reads one command (raw operand values are discarded).
pub fn read_command(r: &mut Reader, depth: u32) -> Result<Command> {
    command_ctx(r, depth, &mut Vec::new())
}

/// Reads a counted command block (raw operand values are discarded).
pub fn read_block(r: &mut Reader, depth: u32) -> Result<Vec<Command>> {
    block_ctx(r, depth, &mut Vec::new())
}

struct Writer<'a> {
    raw: std::slice::Iter<'a, i32>,
    out: Vec<u8>,
}

impl Writer<'_> {
    fn raw(&mut self) -> i32 {
        *self.raw.next().expect("raw_operands exhausted")
    }
    fn u8(&mut self, v: u8) {
        self.out.push(v);
    }
    fn i16(&mut self, v: i16) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }
    fn fixed(&mut self) {
        let v = self.raw();
        self.i32(v);
    }
    fn string(&mut self, s: &str) {
        let b: Vec<u8> = s.chars().map(|c| c as u32 as u8).collect();
        self.u8(b.len() as u8);
        self.out.extend_from_slice(&b);
    }
    fn effect(&mut self, e: &EffectParams) {
        self.i16(e.id);
        self.u8(e.kind);
        for _ in 0..4 {
            self.fixed();
        }
    }
    fn trigger(&mut self, t: &Trigger) {
        use Trigger::*;
        match t {
            EnemyOnTile { enemy, tile } => {
                self.u8(0);
                self.i16(*enemy);
                self.i16(*tile);
            }
            FrogOnTile { frog, tile } => {
                self.u8(1);
                self.u8(*frog);
                self.i16(*tile);
            }
            FrogOnPlatform { frog, platform } => {
                self.u8(2);
                self.u8(*frog);
                self.i16(*platform);
            }
            RandomWait { .. } => {
                self.u8(3);
                self.fixed();
                self.fixed();
            }
            EnemyAtFlag { enemy, flag } => {
                self.u8(8);
                self.i16(*enemy);
                self.i16(*flag);
            }
            PlatAtFlag { platform, flag } => {
                self.u8(9);
                self.i16(*platform);
                self.i16(*flag);
            }
            Wait { .. } => {
                self.u8(10);
                self.fixed();
            }
            FlagSet { flag } => {
                self.u8(11);
                self.u8(*flag);
            }
            CounterEquals { counter, value } => {
                self.u8(12);
                self.u8(*counter);
                self.i16(*value);
            }
            FrogIsDead { frog } => {
                self.u8(13);
                self.u8(*frog);
            }
            LevelIsOpen { world, level } => {
                self.u8(14);
                self.u8(*world);
                self.u8(*level);
            }
            OnPath { frog, enemy } => {
                self.u8(15);
                self.u8(*frog);
                self.i16(*enemy);
            }
            And(a, b) => {
                self.u8(64);
                self.trigger(a);
                self.trigger(b);
            }
            Or(a, b) => {
                self.u8(65);
                self.trigger(a);
                self.trigger(b);
            }
            Not(a) => {
                self.u8(66);
                self.trigger(a);
            }
        }
    }
    fn block(&mut self, cmds: &[Command]) {
        self.i16(cmds.len() as i16);
        for c in cmds {
            self.command(c);
        }
    }
    /// Writes `i32 size` + the block (size excludes itself, includes the count word).
    fn sized(&mut self, cmds: &[Command]) {
        let outer = std::mem::take(&mut self.out);
        self.block(cmds);
        let inner = std::mem::replace(&mut self.out, outer);
        self.i32(inner.len() as i32);
        self.out.extend_from_slice(&inner);
    }
    fn command(&mut self, c: &Command) {
        use Command::*;
        match c {
            Debug { text } => {
                self.u8(0);
                self.string(text);
            }
            SetEnemyFlag { id, flags } => {
                self.u8(2);
                self.i16(*id);
                self.u32(*flags);
            }
            ResetEnemyFlag { id, flags } => {
                self.u8(3);
                self.i16(*id);
                self.u32(*flags);
            }
            SetPlatFlag { id, flags } => {
                self.u8(4);
                self.i16(*id);
                self.u32(*flags);
            }
            ResetPlatFlag { id, flags } => {
                self.u8(5);
                self.i16(*id);
                self.u32(*flags);
            }
            AnimateActor { id, anim, flags, .. } => {
                self.u8(6);
                self.i16(*id);
                self.u8(*anim);
                self.u8(*flags);
                self.fixed();
            }
            SetEnemy { id, value } => {
                self.u8(7);
                self.i16(*id);
                self.u8(*value);
            }
            SetPlatform { id, value } => {
                self.u8(8);
                self.i16(*id);
                self.u8(*value);
            }
            MoveEnemy { id, node } => {
                self.u8(10);
                self.i16(*id);
                self.i16(*node);
            }
            MovePlat { id, node } => {
                self.u8(11);
                self.i16(*id);
                self.i16(*node);
            }
            ChangeVolume { n, platform, .. } => {
                self.u8(13);
                self.u8(*n);
                self.fixed();
                self.i16(*platform);
            }
            ChangeLevel { world, level } => {
                self.u8(16);
                self.u8(*world);
                self.u8(*level);
            }
            Hop { frog, tile } => {
                self.u8(18);
                self.u8(*frog);
                self.i16(*tile);
            }
            Fog { r, g, b, min, max } => {
                self.u8(20);
                self.u8(*r);
                self.u8(*g);
                self.u8(*b);
                self.i16(*min);
                self.i16(*max);
            }
            NoFog => self.u8(21),
            CameoModeOn => self.u8(22),
            CameoModeOff => self.u8(23),
            SetFlag { flag } => {
                self.u8(24);
                self.u8(*flag);
            }
            ResetFlag { flag } => {
                self.u8(25);
                self.u8(*flag);
            }
            SfxEnemy(e) => {
                self.u8(27);
                self.effect(e);
            }
            SfxPlaceholder(e) => {
                self.u8(28);
                self.effect(e);
            }
            HurtFrog => self.u8(29),
            ScaleEnemySpeed { id, .. } => {
                self.u8(30);
                self.i16(*id);
                self.fixed();
            }
            ScalePlatSpeed { id, .. } => {
                self.u8(31);
                self.i16(*id);
                self.fixed();
            }
            IncCounter { counter } => {
                self.u8(32);
                self.u8(*counter);
            }
            DecCounter { counter } => {
                self.u8(33);
                self.u8(*counter);
            }
            SetCounter { counter, value } => {
                self.u8(34);
                self.u8(*counter);
                self.i16(*value);
            }
            CollapsePlat { id, .. } => {
                self.u8(35);
                self.i16(*id);
                self.fixed();
            }
            ChangeFrog { frog, model } => {
                self.u8(36);
                self.u8(*frog);
                self.string(model);
            }
            ShakeCamera { .. } => {
                self.u8(37);
                self.fixed();
                self.fixed();
            }
            SetTileP { id, state } => {
                self.u8(38);
                self.i16(*id);
                self.u8(*state);
            }
            TeleportP { frog, id, .. } => {
                self.u8(39);
                self.u8(*frog);
                self.i16(*id);
                self.fixed();
            }
            SetStartTileP { id } => {
                self.u8(40);
                self.i16(*id);
            }
            SpringP { frog, id, .. } => {
                self.u8(41);
                self.u8(*frog);
                self.i16(*id);
                self.fixed();
                self.fixed();
            }
            PlaySound { name, uid } => {
                self.u8(42);
                self.string(name);
                self.i16(*uid);
            }
            StopSound { name } => {
                self.u8(43);
                self.string(name);
            }
            KillFrog { frog, death } => {
                self.u8(44);
                self.u8(*frog);
                self.u8(*death);
            }
            AnimateFrog { frog, anim, flags, .. } => {
                self.u8(45);
                self.u8(*frog);
                self.u8(*anim);
                self.u8(*flags);
                self.fixed();
            }
            ResetScript => self.u8(46),
            EndLevel => self.u8(47),
            PokeFrogs => self.u8(48),
            SfxPlaceholderNoClip(e) => {
                self.u8(49);
                self.effect(e);
            }
            Hint { string } => {
                self.u8(50);
                self.i16(*string);
            }
            On { trigger, flags, block } => {
                self.u8(64);
                self.trigger(trigger);
                self.u8(*flags);
                self.sized(block);
            }
            If { trigger, block } => {
                self.u8(65);
                self.trigger(trigger);
                self.sized(block);
            }
            LineNumber { line } => {
                self.u8(255);
                let skipped = self.raw();
                self.i16(skipped as i16);
                self.i16(*line);
            }
        }
    }
}

impl Script {
    /// Serialises back to the on-disk format using `raw_operands` for shifted operands.
    /// For a script produced by [`Script::parse`] this reproduces the input bytes
    /// (strings are re-emitted at their NUL-trimmed length).
    pub fn write(&self) -> Vec<u8> {
        let mut w = Writer { raw: self.raw_operands.iter(), out: Vec::new() };
        w.block(&self.commands);
        let body = w.out;
        let mut out = vec![FEV_VERSION];
        out.extend_from_slice(&(body.len() as i32).to_le_bytes());
        out.extend_from_slice(&body);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_and_speed_decode() {
        assert_eq!(fixed_value(0x1234_5678), 0x0123_4567);
        assert_eq!(fixed_value(-16), -1);
        assert_eq!(speed_value(0x0001_2345), 0x123);
    }

    #[test]
    fn roundtrip_synthetic_keeps_low_bits() {
        let mut b = vec![FEV_VERSION, 0, 0, 0, 0];
        let mut inner = vec![1, 0, 35];
        inner.extend_from_slice(&7i16.to_le_bytes());
        inner.extend_from_slice(&0x1239i32.to_le_bytes());
        let mut body = vec![2, 0, 64, 10];
        body.extend_from_slice(&0x0ABCDi32.to_le_bytes());
        body.push(3);
        body.extend_from_slice(&(inner.len() as i32).to_le_bytes());
        body.extend_from_slice(&inner);
        body.push(255);
        body.extend_from_slice(&5i16.to_le_bytes());
        body.extend_from_slice(&42i16.to_le_bytes());
        let n = body.len() as i32;
        b[1..5].copy_from_slice(&n.to_le_bytes());
        b.extend_from_slice(&body);

        let s = Script::parse(&b).unwrap();
        assert_eq!(s.raw_operands, vec![0x0ABCD, 0x1239, 5]);
        match &s.commands[0] {
            Command::On { trigger: Trigger::Wait { time }, block, .. } => {
                assert_eq!(*time, 0xABC);
                assert_eq!(block[0], Command::CollapsePlat { id: 7, time: 0x123 });
            }
            c => panic!("{c:?}"),
        }
        assert_eq!(s.write(), b);
    }

    #[test]
    fn real_scripts_roundtrip() {
        let Some(dir) = std::env::var_os("F2_DATA_DIR").or_else(|| std::env::var_os("F2_GAME_DIR")) else { return };
        let maps = std::path::Path::new(&dir).join("maps");
        let mut n = 0;
        for e in std::fs::read_dir(maps).unwrap().flatten() {
            if e.path().extension().is_some_and(|x| x == "fev") {
                let raw = std::fs::read(e.path()).unwrap();
                let s = Script::parse(&raw).unwrap();
                assert_eq!(s.write(), raw, "{:?}", e.path());
                n += 1;
            }
        }
        eprintln!("round-tripped {n} scripts");
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_json_roundtrip() {
        let s = Script { commands: vec![Command::Hop { frog: 1, tile: 2 }, Command::NoFog], raw_operands: vec![] };
        let j = serde_json::to_string(&s).unwrap();
        let back: Script = serde_json::from_str(&j).unwrap();
        assert_eq!(back.commands, s.commands);
    }
}
