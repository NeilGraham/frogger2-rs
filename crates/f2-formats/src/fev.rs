//! Compiled event scripts `maps/script-<world>-<level>.fev` (ETC output).
//!
//! Decoded into a tree mirroring `ExecuteCommand` / `LoadTrigger`
//! (`Common/script.c`). Values are kept as stored, except `FIXED` fields
//! which are `int >> 4` like `MEMGETFIXED`. Opcodes from `Common/codes.h`.
//!
//! File: `u8 version (=3)`, `i32 outer block size`, block.
//! Block: `i16 count`, `count` commands.

use crate::read::{cstr, Reader, Result};

pub const FEV_VERSION: u8 = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trigger {
    EnemyOnTile {
        enemy: i16,
        tile: i16,
    },
    FrogOnTile {
        frog: u8,
        tile: i16,
    },
    FrogOnPlatform {
        frog: u8,
        platform: i16,
    },
    RandomWait {
        start: i32,
        end: i32,
    },
    EnemyAtFlag {
        enemy: i16,
        flag: i16,
    },
    PlatAtFlag {
        platform: i16,
        flag: i16,
    },
    /// Seconds in 20.12 fixed point.
    Wait {
        time: i32,
    },
    FlagSet {
        flag: u8,
    },
    CounterEquals {
        counter: u8,
        value: i16,
    },
    FrogIsDead {
        frog: u8,
    },
    LevelIsOpen {
        world: u8,
        level: u8,
    },
    OnPath {
        frog: u8,
        enemy: i16,
    },
    And(Box<Trigger>, Box<Trigger>),
    Or(Box<Trigger>, Box<Trigger>),
    Not(Box<Trigger>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectParams {
    pub id: i16,
    pub kind: u8,
    pub size: i32,
    pub speed: i32,
    pub accn: i32,
    pub lifetime: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// `EV_ON`: attach `block` to run when `trigger` fires (`flags` = TRIGGER_*).
    On {
        trigger: Trigger,
        flags: u8,
        block: Vec<Command>,
    },
    /// `EV_IF`: run `block` now if `trigger` holds.
    If {
        trigger: Trigger,
        block: Vec<Command>,
    },
    SetEnemyFlag {
        id: i16,
        flags: u32,
    },
    ResetEnemyFlag {
        id: i16,
        flags: u32,
    },
    SetPlatFlag {
        id: i16,
        flags: u32,
    },
    ResetPlatFlag {
        id: i16,
        flags: u32,
    },
    AnimateActor {
        id: i16,
        anim: u8,
        flags: u8,
        speed: i32,
    },
    SetEnemy {
        id: i16,
        value: u8,
    },
    SetPlatform {
        id: i16,
        value: u8,
    },
    MoveEnemy {
        id: i16,
        node: i16,
    },
    MovePlat {
        id: i16,
        node: i16,
    },
    ChangeVolume {
        n: u8,
        delta: i32,
        platform: i16,
    },
    ChangeLevel {
        world: u8,
        level: u8,
    },
    Hop {
        frog: u8,
        tile: i16,
    },
    Fog {
        r: u8,
        g: u8,
        b: u8,
        min: i16,
        max: i16,
    },
    NoFog,
    CameoModeOn,
    CameoModeOff,
    SetFlag {
        flag: u8,
    },
    ResetFlag {
        flag: u8,
    },
    SfxEnemy(EffectParams),
    SfxPlaceholder(EffectParams),
    SfxPlaceholderNoClip(EffectParams),
    HurtFrog,
    ScaleEnemySpeed {
        id: i16,
        speed: i32,
    },
    ScalePlatSpeed {
        id: i16,
        speed: i32,
    },
    IncCounter {
        counter: u8,
    },
    DecCounter {
        counter: u8,
    },
    SetCounter {
        counter: u8,
        value: i16,
    },
    CollapsePlat {
        id: i16,
        time: i32,
    },
    ChangeFrog {
        frog: u8,
        model: String,
    },
    ShakeCamera {
        shakiness: i32,
        falloff: i32,
    },
    SetTileP {
        id: i16,
        state: u8,
    },
    SetStartTileP {
        id: i16,
    },
    TeleportP {
        frog: u8,
        id: i16,
        time: i32,
    },
    SpringP {
        frog: u8,
        id: i16,
        height: i32,
        time: i32,
    },
    PlaySound {
        name: String,
        uid: i16,
    },
    StopSound {
        name: String,
    },
    AnimateFrog {
        frog: u8,
        anim: u8,
        flags: u8,
        speed: i32,
    },
    KillFrog {
        frog: u8,
        death: u8,
    },
    EndLevel,
    ResetScript,
    PokeFrogs,
    Hint {
        string: i16,
    },
    Debug {
        text: String,
    },
    /// `C_DEBUG` line marker (debug builds of ETC).
    LineNumber {
        line: i16,
    },
}

#[derive(Clone, Debug)]
pub struct Script {
    pub commands: Vec<Command>,
}

fn fixed(r: &mut Reader) -> Result<i32> {
    Ok(r.i32()? >> 4)
}

fn string(r: &mut Reader) -> Result<String> {
    let n = r.u8()? as usize;
    Ok(cstr(r.bytes(n)?))
}

pub fn read_trigger(r: &mut Reader, depth: u32) -> Result<Trigger> {
    if depth > 64 {
        return Err(r.err("trigger nesting too deep"));
    }
    let t = r.u8()?;
    Ok(match t {
        0 => Trigger::EnemyOnTile { enemy: r.i16()?, tile: r.i16()? },
        1 => Trigger::FrogOnTile { frog: r.u8()?, tile: r.i16()? },
        2 => Trigger::FrogOnPlatform { frog: r.u8()?, platform: r.i16()? },
        3 => Trigger::RandomWait { start: fixed(r)?, end: fixed(r)? },
        8 => Trigger::EnemyAtFlag { enemy: r.i16()?, flag: r.i16()? },
        9 => Trigger::PlatAtFlag { platform: r.i16()?, flag: r.i16()? },
        10 => Trigger::Wait { time: fixed(r)? },
        11 => Trigger::FlagSet { flag: r.u8()? },
        12 => Trigger::CounterEquals { counter: r.u8()?, value: r.i16()? },
        13 => Trigger::FrogIsDead { frog: r.u8()? },
        14 => Trigger::LevelIsOpen { world: r.u8()?, level: r.u8()? },
        15 => Trigger::OnPath { frog: r.u8()?, enemy: r.i16()? },
        64 => Trigger::And(Box::new(read_trigger(r, depth + 1)?), Box::new(read_trigger(r, depth + 1)?)),
        65 => Trigger::Or(Box::new(read_trigger(r, depth + 1)?), Box::new(read_trigger(r, depth + 1)?)),
        66 => Trigger::Not(Box::new(read_trigger(r, depth + 1)?)),
        x => return Err(r.err(format!("unknown trigger {x}"))),
    })
}

fn effect(r: &mut Reader) -> Result<EffectParams> {
    Ok(EffectParams { id: r.i16()?, kind: r.u8()?, size: fixed(r)?, speed: fixed(r)?, accn: fixed(r)?, lifetime: fixed(r)? })
}

/// Reads a sized sub-block and checks it ends exactly at `size`.
fn sized_block(r: &mut Reader, size: usize, depth: u32) -> Result<Vec<Command>> {
    let start = r.pos;
    let bytes = r.bytes(size)?;
    let mut sub = Reader::at(r.what, &r.data[..start + bytes.len()], start);
    let block = read_block(&mut sub, depth + 1)?;
    if sub.pos != start + size {
        return Err(sub.err(format!("block size {size} but commands used {}", sub.pos - start)));
    }
    Ok(block)
}

pub fn read_command(r: &mut Reader, depth: u32) -> Result<Command> {
    let op = r.u8()?;
    Ok(match op {
        0 => Command::Debug { text: string(r)? },
        2 => Command::SetEnemyFlag { id: r.i16()?, flags: r.u32()? },
        3 => Command::ResetEnemyFlag { id: r.i16()?, flags: r.u32()? },
        4 => Command::SetPlatFlag { id: r.i16()?, flags: r.u32()? },
        5 => Command::ResetPlatFlag { id: r.i16()?, flags: r.u32()? },
        6 => Command::AnimateActor { id: r.i16()?, anim: r.u8()?, flags: r.u8()?, speed: r.i32()? >> 8 },
        7 => Command::SetEnemy { id: r.i16()?, value: r.u8()? },
        8 => Command::SetPlatform { id: r.i16()?, value: r.u8()? },
        10 => Command::MoveEnemy { id: r.i16()?, node: r.i16()? },
        11 => Command::MovePlat { id: r.i16()?, node: r.i16()? },
        13 => Command::ChangeVolume { n: r.u8()?, delta: fixed(r)?, platform: r.i16()? },
        16 => Command::ChangeLevel { world: r.u8()?, level: r.u8()? },
        18 => Command::Hop { frog: r.u8()?, tile: r.i16()? },
        20 => Command::Fog { r: r.u8()?, g: r.u8()?, b: r.u8()?, min: r.i16()?, max: r.i16()? },
        21 => Command::NoFog,
        22 => Command::CameoModeOn,
        23 => Command::CameoModeOff,
        24 => Command::SetFlag { flag: r.u8()? },
        25 => Command::ResetFlag { flag: r.u8()? },
        27 => Command::SfxEnemy(effect(r)?),
        28 => Command::SfxPlaceholder(effect(r)?),
        29 => Command::HurtFrog,
        30 => Command::ScaleEnemySpeed { id: r.i16()?, speed: fixed(r)? },
        31 => Command::ScalePlatSpeed { id: r.i16()?, speed: fixed(r)? },
        32 => Command::IncCounter { counter: r.u8()? },
        33 => Command::DecCounter { counter: r.u8()? },
        34 => Command::SetCounter { counter: r.u8()?, value: r.i16()? },
        35 => Command::CollapsePlat { id: r.i16()?, time: fixed(r)? },
        36 => Command::ChangeFrog { frog: r.u8()?, model: string(r)? },
        37 => Command::ShakeCamera { shakiness: fixed(r)?, falloff: fixed(r)? },
        38 => Command::SetTileP { id: r.i16()?, state: r.u8()? },
        39 => Command::TeleportP { frog: r.u8()?, id: r.i16()?, time: fixed(r)? },
        40 => Command::SetStartTileP { id: r.i16()? },
        41 => Command::SpringP { frog: r.u8()?, id: r.i16()?, height: fixed(r)?, time: fixed(r)? },
        42 => Command::PlaySound { name: string(r)?, uid: r.i16()? },
        43 => Command::StopSound { name: string(r)? },
        44 => Command::KillFrog { frog: r.u8()?, death: r.u8()? },
        45 => Command::AnimateFrog { frog: r.u8()?, anim: r.u8()?, flags: r.u8()?, speed: r.i32()? >> 8 },
        46 => Command::ResetScript,
        47 => Command::EndLevel,
        48 => Command::PokeFrogs,
        49 => Command::SfxPlaceholderNoClip(effect(r)?),
        50 => Command::Hint { string: r.i16()? },
        64 => {
            let trigger = read_trigger(r, 0)?;
            let flags = r.u8()?;
            let size = r.i32()?;
            if size < 0 {
                return Err(r.err("negative block size"));
            }
            Command::On { trigger, flags, block: sized_block(r, size as usize, depth)? }
        }
        65 => {
            let trigger = read_trigger(r, 0)?;
            let size = r.i32()?;
            if size < 0 {
                return Err(r.err("negative block size"));
            }
            Command::If { trigger, block: sized_block(r, size as usize, depth)? }
        }
        255 => {
            r.i16()?;
            Command::LineNumber { line: r.i16()? }
        }
        x => return Err(r.err(format!("unknown command {x}"))),
    })
}

pub fn read_block(r: &mut Reader, depth: u32) -> Result<Vec<Command>> {
    if depth > 64 {
        return Err(r.err("block nesting too deep"));
    }
    let n = r.i16()?;
    if n < 0 {
        return Err(r.err(format!("negative command count {n}")));
    }
    (0..n).map(|_| read_command(r, depth)).collect()
}

impl Script {
    pub fn parse(data: &[u8]) -> Result<Script> {
        let mut r = Reader::new("fev", data);
        let v = r.u8()?;
        if v != FEV_VERSION {
            return Err(r.err(format!("script version {v}, expected {FEV_VERSION}")));
        }
        let _outer = r.i32()?;
        let commands = read_block(&mut r, 0)?;
        Ok(Script { commands })
    }
}
