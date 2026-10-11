//! Stack VM for compiled F2S [`Program`]s. Every run is bounded: an instruction budget
//! (shared by all hook calls of a script within one tick), a value-stack limit, a call-depth
//! limit and a string-length limit. Nothing persists between runs except the globals.

use crate::prelude::*;

use serde::Serialize;

use super::lang::{Op, Program};

/// A script value: a 32-bit integer (wrapping arithmetic) or an immutable string.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum Value {
    Int(i32),
    Str(String),
}

impl Default for Value {
    fn default() -> Self {
        Value::Int(0)
    }
}

impl Value {
    pub fn truthy(&self) -> bool {
        match self {
            Value::Int(i) => *i != 0,
            Value::Str(s) => !s.is_empty(),
        }
    }
}

impl core::fmt::Display for Value {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Value::Int(i) => write!(f, "{i}"),
            Value::Str(s) => f.write_str(s),
        }
    }
}

/// Instructions one script may execute per tick (all hooks together).
pub const STEPS_PER_TICK: u32 = 200_000;
/// Value stack limit (locals + temporaries of all active calls).
pub const MAX_STACK: usize = 4096;
/// Nested script function calls.
pub const MAX_DEPTH: usize = 64;
/// Longest string a script can build (bytes).
pub const MAX_STRING: usize = 1024;

/// Why a run stopped early. Either way the script is disabled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VmError {
    /// The per-tick instruction budget ran out.
    Budget,
    Runtime(String),
}

impl core::fmt::Display for VmError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            VmError::Budget => write!(f, "instruction budget exceeded ({STEPS_PER_TICK} per tick)"),
            VmError::Runtime(s) => f.write_str(s),
        }
    }
}

/// The game side of the natives.
pub trait Host {
    fn native(&mut self, id: u16, args: &[Value]) -> Result<Value, String>;
}

struct Frame {
    ret: usize,
    base: usize,
    func: u16,
}

fn int(v: &Value, what: &str) -> Result<i32, VmError> {
    match v {
        Value::Int(i) => Ok(*i),
        Value::Str(_) => Err(VmError::Runtime(format!("{what}: expected a number, got a string"))),
    }
}

/// Runs function `func` with `args`. `steps` is the remaining budget (decremented).
pub fn run(prog: &Program, globals: &mut [Value], func: u16, args: &[Value], steps: &mut u32, host: &mut dyn Host) -> Result<Value, VmError> {
    let f = &prog.funcs[func as usize];
    if args.len() != f.arity {
        return Err(VmError::Runtime(format!("{} called with {} arguments", f.name, args.len())));
    }
    let mut stack: Vec<Value> = Vec::with_capacity(64);
    stack.extend_from_slice(args);
    stack.resize(f.n_locals, Value::Int(0));
    let mut frames = vec![Frame { ret: usize::MAX, base: 0, func }];
    let mut pc = f.entry as usize;
    let rt = |m: String| VmError::Runtime(m);
    macro_rules! pop {
        () => {
            stack.pop().ok_or_else(|| rt("stack underflow".into()))?
        };
    }
    loop {
        if *steps == 0 {
            return Err(VmError::Budget);
        }
        *steps -= 1;
        let op = *prog.code.get(pc).ok_or_else(|| rt("pc out of range".into()))?;
        pc += 1;
        let base = frames.last().expect("frame").base;
        match op {
            Op::Int(i) => stack.push(Value::Int(i)),
            Op::Str(s) => stack.push(Value::Str(prog.strings[s as usize].clone())),
            Op::LoadG(g) => stack.push(globals[g as usize].clone()),
            Op::StoreG(g) => globals[g as usize] = pop!(),
            Op::LoadL(l) => stack.push(stack[base + l as usize].clone()),
            Op::StoreL(l) => {
                let v = pop!();
                stack[base + l as usize] = v;
            }
            Op::Pop => {
                pop!();
            }
            Op::Add => {
                let b = pop!();
                let a = pop!();
                stack.push(match (a, b) {
                    (Value::Int(a), Value::Int(b)) => Value::Int(a.wrapping_add(b)),
                    (a, b) => {
                        let s = format!("{a}{b}");
                        if s.len() > MAX_STRING {
                            return Err(rt(format!("string longer than {MAX_STRING} bytes")));
                        }
                        Value::Str(s)
                    }
                });
            }
            Op::Eq | Op::Ne => {
                let b = pop!();
                let a = pop!();
                stack.push(Value::Int(((a == b) == (op == Op::Eq)) as i32));
            }
            Op::Sub | Op::Mul | Op::Div | Op::Rem | Op::Shl | Op::Shr | Op::And | Op::Or | Op::Xor | Op::Lt | Op::Le | Op::Gt | Op::Ge => {
                let b = int(&pop!(), "operator")?;
                let a = int(&pop!(), "operator")?;
                let v = match op {
                    Op::Sub => a.wrapping_sub(b),
                    Op::Mul => a.wrapping_mul(b),
                    Op::Div | Op::Rem if b == 0 => return Err(rt("division by zero".into())),
                    Op::Div => a.wrapping_div(b),
                    Op::Rem => a.wrapping_rem(b),
                    Op::Shl => a.wrapping_shl(b as u32 & 31),
                    Op::Shr => a.wrapping_shr(b as u32 & 31),
                    Op::And => a & b,
                    Op::Or => a | b,
                    Op::Xor => a ^ b,
                    Op::Lt => (a < b) as i32,
                    Op::Le => (a <= b) as i32,
                    Op::Gt => (a > b) as i32,
                    _ => (a >= b) as i32,
                };
                stack.push(Value::Int(v));
            }
            Op::Neg => {
                let a = int(&pop!(), "-")?;
                stack.push(Value::Int(a.wrapping_neg()));
            }
            Op::BitNot => {
                let a = int(&pop!(), "~")?;
                stack.push(Value::Int(!a));
            }
            Op::Not => {
                let a = pop!();
                stack.push(Value::Int(!a.truthy() as i32));
            }
            Op::Jmp(t) => pc = t as usize,
            Op::Jz(t) => {
                if !pop!().truthy() {
                    pc = t as usize;
                }
            }
            Op::Jnz(t) => {
                if pop!().truthy() {
                    pc = t as usize;
                }
            }
            Op::Call(fi) => {
                let f = &prog.funcs[fi as usize];
                if frames.len() >= MAX_DEPTH {
                    return Err(rt(format!("call depth over {MAX_DEPTH} (runaway recursion in {}?)", f.name)));
                }
                let nb = stack.len() - f.arity;
                if nb + f.n_locals > MAX_STACK {
                    return Err(rt("stack overflow".into()));
                }
                stack.resize(nb + f.n_locals, Value::Int(0));
                frames.push(Frame { ret: pc, base: nb, func: fi });
                pc = f.entry as usize;
            }
            Op::Native(id) => {
                let n = super::api::Native::from_id(id).arity();
                let at = stack.len() - n;
                let r = host.native(id, &stack[at..]).map_err(|e| rt(format!("{}: {e}", super::api::Native::from_id(id).name())))?;
                if let Value::Str(s) = &r {
                    if s.len() > MAX_STRING {
                        return Err(rt(format!("string longer than {MAX_STRING} bytes")));
                    }
                }
                stack.truncate(at);
                stack.push(r);
            }
            Op::Ret => {
                let v = pop!();
                let fr = frames.pop().expect("frame");
                if frames.is_empty() {
                    return Ok(v);
                }
                let _ = fr.func;
                stack.truncate(fr.base);
                stack.push(v);
                pc = fr.ret;
            }
        }
        if stack.len() > MAX_STACK {
            return Err(rt("stack overflow".into()));
        }
    }
}
