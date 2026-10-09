//! `.fev` decoder and decompiler (new; not part of the original ETC).
//!
//! Decoding is driven by the same command/trigger tables the compiler uses,
//! and keeps every operand exactly as stored so that the printed source
//! recompiles to the identical bytes.

use crate::compiler::{float_to_fixed, ETC_VERSION};
use crate::lexer::c_atof;
use crate::table::{ScriptTokenList, C_DEBUG, C_ON};
use crate::table::{PARAM_BLOCK, PARAM_BYTE, PARAM_FLOAT, PARAM_INT, PARAM_NONE, PARAM_STRING, PARAM_TRIGGER, PARAM_WORD};

/// One operand as stored in the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Arg {
    Byte(u8),
    Word(i16),
    Int(i32),
    /// 16.16 fixed point as written by `Buffer::AddFloat`.
    Float(i32),
    Str(Vec<u8>),
    Trigger(Trigger),
    Block(Vec<Event>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trigger {
    pub token: u8,
    pub args: Vec<Arg>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// `On <trigger> <flags> <block>`.
    On { trigger: Trigger, flags: u8, block: Vec<Event> },
    /// Any table command (including `If`).
    Command { token: u8, args: Vec<Arg> },
    /// `C_DEBUG` record from `etc -d` (cannot be written in source).
    DebugLine { pad: i16, line: i16 },
}

/// A decoded `.fev` file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fev {
    pub events: Vec<Event>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodeError {
    pub offset: usize,
    pub message: String,
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "offset 0x{:x}: {}", self.offset, self.message)
    }
}

impl std::error::Error for DecodeError {}

type R<T> = Result<T, DecodeError>;

struct Decoder<'a> {
    d: &'a [u8],
    p: usize,
    commands: &'a ScriptTokenList,
    triggers: &'a ScriptTokenList,
    depth: u32,
}

impl Decoder<'_> {
    fn err<T>(&self, m: impl Into<String>) -> R<T> {
        Err(DecodeError { offset: self.p, message: m.into() })
    }
    fn take(&mut self, n: usize) -> R<&[u8]> {
        if self.p + n > self.d.len() {
            return self.err("unexpected end of file");
        }
        let s = &self.d[self.p..self.p + n];
        self.p += n;
        Ok(s)
    }
    fn u8(&mut self) -> R<u8> {
        Ok(self.take(1)?[0])
    }
    fn i16(&mut self) -> R<i16> {
        let b = self.take(2)?;
        Ok(i16::from_le_bytes([b[0], b[1]]))
    }
    fn i32(&mut self) -> R<i32> {
        let b = self.take(4)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn params(&mut self, params: &[u8]) -> R<Vec<Arg>> {
        let mut out = Vec::new();
        for &t in params {
            out.push(match t {
                PARAM_NONE => break,
                PARAM_BYTE => Arg::Byte(self.u8()?),
                PARAM_WORD => Arg::Word(self.i16()?),
                PARAM_INT => Arg::Int(self.i32()?),
                PARAM_FLOAT => Arg::Float(self.i32()?),
                PARAM_STRING => {
                    let n = self.u8()? as usize;
                    Arg::Str(self.take(n)?.to_vec())
                }
                PARAM_TRIGGER => Arg::Trigger(self.trigger()?),
                PARAM_BLOCK => Arg::Block(self.sized_block()?),
                x => return self.err(format!("bad param type {x} in table")),
            });
        }
        Ok(out)
    }

    fn trigger(&mut self) -> R<Trigger> {
        self.depth += 1;
        if self.depth > 256 {
            return self.err("nesting too deep");
        }
        let at = self.p;
        let token = self.u8()?;
        let Some(info) = self.triggers.by_token(token as i32) else {
            self.p = at;
            return self.err(format!("unknown trigger {token}"));
        };
        let params = info.params.clone();
        let args = self.params(&params)?;
        self.depth -= 1;
        Ok(Trigger { token, args })
    }

    /// `i32 size, i16 count, commands`, size covering count + commands.
    fn sized_block(&mut self) -> R<Vec<Event>> {
        self.depth += 1;
        if self.depth > 256 {
            return self.err("nesting too deep");
        }
        let size = self.i32()?;
        let start = self.p;
        let events = self.block()?;
        if size < 0 || self.p - start != size as usize {
            return Err(DecodeError { offset: start - 4, message: format!("block size field {size} but block is {} bytes", self.p - start) });
        }
        self.depth -= 1;
        Ok(events)
    }

    fn block(&mut self) -> R<Vec<Event>> {
        let n = self.i16()?;
        if n < 0 {
            return self.err(format!("negative command count {n}"));
        }
        (0..n).map(|_| self.event()).collect()
    }

    fn event(&mut self) -> R<Event> {
        let at = self.p;
        let op = self.u8()?;
        if op as i32 == C_DEBUG {
            return Ok(Event::DebugLine { pad: self.i16()?, line: self.i16()? });
        }
        if op as i32 == C_ON {
            let trigger = self.trigger()?;
            let flags = self.u8()?;
            let block = self.sized_block()?;
            return Ok(Event::On { trigger, flags, block });
        }
        let Some(info) = self.commands.by_token(op as i32) else {
            self.p = at;
            return self.err(format!("unknown command {op}"));
        };
        let params = info.params.clone();
        Ok(Event::Command { token: op, args: self.params(&params)? })
    }
}

/// Decodes a `.fev` image. Fails on anything the compiler could not have
/// produced structurally (bad sizes, unknown opcodes, trailing bytes).
pub fn decode(data: &[u8], commands: &ScriptTokenList, triggers: &ScriptTokenList) -> R<Fev> {
    let mut d = Decoder { d: data, p: 0, commands, triggers, depth: 0 };
    let v = d.u8()?;
    if v != ETC_VERSION {
        return Err(DecodeError { offset: 0, message: format!("version {v}, expected {ETC_VERSION}") });
    }
    let events = d.sized_block()?;
    if d.p != data.len() {
        return d.err(format!("{} trailing bytes", data.len() - d.p));
    }
    Ok(Fev { events })
}

/// Shortest decimal text that `AddFloat` turns back into `raw`.
pub fn float_text(raw: i32) -> Option<String> {
    let x = raw as f64 / 65536.0;
    for prec in 0..=20 {
        let s = format!("{x:.prec$}");
        if float_to_fixed(c_atof(s.as_bytes())) == raw {
            let s = if s.contains('.') { s } else { format!("{s}.0") };
            return Some(s);
        }
    }
    None
}

/// Named trigger flags (`TRIGGER_*`, `Common/event.h`) emitted as `Set` variables.
pub const TRIGGER_FLAGS: [(&str, u8); 5] = [("TRIGGER_ONCE", 1), ("TRIGGER_DELAY", 2), ("TRIGGER_ALWAYS", 4), ("TRIGGER_FIRED", 8), ("TRIGGER_RISING", 16)];

/// Decompiler output.
#[derive(Clone, Debug)]
pub struct Decompiled {
    /// Source text (bytes: strings are copied verbatim).
    pub source: Vec<u8>,
    /// True when the output contains `C_DEBUG` records that were turned into
    /// comments, so it will not recompile to identical bytes.
    pub lossy: bool,
}

struct Printer<'a> {
    out: Vec<u8>,
    commands: &'a ScriptTokenList,
    triggers: &'a ScriptTokenList,
    lossy: bool,
}

impl Printer<'_> {
    fn indent(&mut self, n: usize) {
        for _ in 0..n {
            self.out.extend_from_slice(b"    ");
        }
    }
    fn s(&mut self, s: &str) {
        self.out.extend_from_slice(s.as_bytes());
    }

    fn arg(&mut self, a: &Arg, ind: usize) -> Result<(), String> {
        match a {
            Arg::Byte(v) => self.s(&v.to_string()),
            Arg::Word(v) => self.s(&v.to_string()),
            Arg::Int(v) => self.s(&v.to_string()),
            Arg::Float(v) => {
                let t = float_text(*v).ok_or_else(|| format!("fixed value {v} cannot be written as a number"))?;
                self.s(&t)
            }
            Arg::Str(b) => {
                if let Some(c) = b.iter().find(|&&c| matches!(c, b'"' | b';' | b'\n' | b'\r' | 0 | 0xFF)) {
                    return Err(format!("string {:?} contains byte 0x{c:02x}, which ETC strings cannot hold", String::from_utf8_lossy(b)));
                }
                self.out.push(b'"');
                self.out.extend_from_slice(b);
                self.out.push(b'"');
            }
            Arg::Trigger(t) => self.trigger(t, ind)?,
            Arg::Block(evs) => self.braced(evs, ind)?,
        }
        Ok(())
    }

    fn args(&mut self, args: &[Arg], ind: usize) -> Result<(), String> {
        self.s("(");
        for (i, a) in args.iter().enumerate() {
            if i > 0 {
                self.s(", ");
            }
            self.arg(a, ind)?;
        }
        self.s(")");
        Ok(())
    }

    fn trigger(&mut self, t: &Trigger, ind: usize) -> Result<(), String> {
        let name = self.triggers.by_token(t.token as i32).map(|e| e.name.clone()).ok_or("unknown trigger")?;
        self.s(&name);
        self.args(&t.args, ind)
    }

    /// `{` newline, events, `}` (no trailing newline).
    fn braced(&mut self, evs: &[Event], ind: usize) -> Result<(), String> {
        self.s("{\n");
        for e in evs {
            self.event(e, ind + 1)?;
        }
        self.indent(ind);
        self.s("}");
        Ok(())
    }

    fn event(&mut self, e: &Event, ind: usize) -> Result<(), String> {
        self.indent(ind);
        match e {
            Event::On { trigger, flags, block } => {
                let on = self.commands.by_token(C_ON).map(|e| e.name.clone()).ok_or("no On command in table")?;
                self.s(&on);
                self.s(" ");
                self.trigger(trigger, ind)?;
                self.s(" ");
                match TRIGGER_FLAGS.iter().find(|f| f.1 == *flags) {
                    Some((n, _)) => self.s(&format!("%{n}")),
                    None => self.s(&flags.to_string()),
                }
                self.s(" ");
                self.braced(block, ind)?;
            }
            Event::Command { token, args } => {
                let name = self.commands.by_token(*token as i32).map(|e| e.name.clone()).ok_or("unknown command")?;
                self.s(&name);
                self.args(args, ind)?;
            }
            Event::DebugLine { pad, line } => {
                self.lossy = true;
                self.s(&format!("; [C_DEBUG {pad} {line}]"));
            }
        }
        self.s("\n");
        Ok(())
    }
}

fn used_flags(evs: &[Event], used: &mut [bool; 5]) {
    fn args(a: &[Arg], used: &mut [bool; 5]) {
        for x in a {
            if let Arg::Block(b) = x {
                used_flags(b, used);
            }
        }
    }
    for e in evs {
        match e {
            Event::On { flags, block, .. } => {
                if let Some(i) = TRIGGER_FLAGS.iter().position(|f| f.1 == *flags) {
                    used[i] = true;
                }
                used_flags(block, used);
            }
            Event::Command { args: a, .. } => args(a, used),
            Event::DebugLine { .. } => {}
        }
    }
}

/// Prints ETC source for a decoded script.
pub fn print(fev: &Fev, commands: &ScriptTokenList, triggers: &ScriptTokenList, title: &str) -> Result<Decompiled, String> {
    let mut p = Printer { out: Vec::new(), commands, triggers, lossy: false };
    p.s(&format!("; {title}\n; Decompiled by f2-etc. Recompile with: f2-etc compile <this file>\n\n"));
    let mut used = [false; 5];
    used_flags(&fev.events, &mut used);
    if used.iter().any(|&u| u) {
        p.s("; Trigger flags (Common/event.h)\n");
        for (i, (n, v)) in TRIGGER_FLAGS.iter().enumerate() {
            if used[i] {
                p.s(&format!("Set %{n} {v}\n"));
            }
        }
        p.s("\n");
    }
    for (i, e) in fev.events.iter().enumerate() {
        if i > 0 && matches!(e, Event::On { .. }) {
            p.s("\n");
        }
        p.event(e, 0)?;
    }
    Ok(Decompiled { source: p.out, lossy: p.lossy })
}
