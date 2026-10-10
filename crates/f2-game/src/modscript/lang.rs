//! The F2S behaviour-script language: lexer and single-pass compiler to [`Program`]
//! bytecode (see `docs/modding.md` §4 for the language reference).
//!
//! Integers only (`i32`, wrapping), plus immutable strings for messages and names. No
//! floats, no I/O, no clock: a program can only reach the game through the natives of
//! [`super::api`].

use std::collections::BTreeMap;

use super::api::{builtin_const, Native};
use super::vm::Value;

/// Hook functions a script may define: `(name, arity)`, indexed by [`Hook`].
pub const HOOKS: [(&str, usize); 6] =
    [("on_level_start", 2), ("on_tick", 0), ("on_frog_land", 3), ("on_frog_death", 2), ("on_pickup", 2), ("on_enemy_update", 1)];

/// Index into [`HOOKS`] / [`Program::hooks`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hook {
    LevelStart = 0,
    Tick = 1,
    FrogLand = 2,
    FrogDeath = 3,
    Pickup = 4,
    EnemyUpdate = 5,
}

/// Source size limit (bytes).
pub const MAX_SOURCE: usize = 256 * 1024;
/// Limits on declarations (keep state small and indices in range).
pub const MAX_GLOBALS: usize = 1024;
pub const MAX_LOCALS: usize = 256;
pub const MAX_PARAMS: usize = 16;

/// Bytecode instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Int(i32),
    Str(u32),
    LoadG(u16),
    StoreG(u16),
    LoadL(u16),
    StoreL(u16),
    Pop,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Shl,
    Shr,
    And,
    Or,
    Xor,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Neg,
    Not,
    BitNot,
    Jmp(u32),
    /// Pop; jump if falsy.
    Jz(u32),
    /// Pop; jump if truthy.
    Jnz(u32),
    Call(u16),
    Native(u16),
    Ret,
}

/// A compiled function.
#[derive(Clone, Debug)]
pub struct Func {
    pub name: String,
    pub arity: usize,
    /// Parameters + every `let` in the body.
    pub n_locals: usize,
    pub entry: u32,
}

/// A compiled script (immutable; shared behind an `Arc`).
#[derive(Clone, Debug, Default)]
pub struct Program {
    pub code: Vec<Op>,
    pub funcs: Vec<Func>,
    pub strings: Vec<String>,
    pub globals: Vec<String>,
    /// Global initialisers (`let` at top level), a function of arity 0.
    pub init: u16,
    pub hooks: [Option<u16>; HOOKS.len()],
}

impl Program {
    pub fn hook(&self, h: Hook) -> Option<u16> {
        self.hooks[h as usize]
    }
}

/// A compile error with its 1-based source line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompileError {
    pub line: u32,
    pub msg: String,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.msg)
    }
}

// ------------------------------------------------------------------ lexer

#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    Int(i32),
    Str(String),
    Ident(String),
    Punct(&'static str),
    Eof,
}

const PUNCTS: [&str; 33] = [
    "<<=", ">>=", "==", "!=", "<=", ">=", "<<", ">>", "&&", "||", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "(", ")", "{", "}", ",", ";", "=", "<", ">",
    "+", "-", "*", "/", "%", "!",
];
const PUNCTS1: [&str; 4] = ["&", "|", "^", "~"];

fn lex(src: &str) -> Result<Vec<(Tok, u32)>, CompileError> {
    let b = src.as_bytes();
    let mut i = 0;
    let mut line = 1u32;
    let mut out = Vec::new();
    let err = |line: u32, msg: String| CompileError { line, msg };
    while i < b.len() {
        let c = b[i];
        if c == b'\n' {
            line += 1;
            i += 1;
        } else if c.is_ascii_whitespace() {
            i += 1;
        } else if b[i..].starts_with(b"//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if b[i..].starts_with(b"/*") {
            let start = line;
            i += 2;
            loop {
                if i + 1 >= b.len() {
                    return Err(err(start, "unterminated comment".into()));
                }
                if b[i] == b'*' && b[i + 1] == b'/' {
                    i += 2;
                    break;
                }
                if b[i] == b'\n' {
                    line += 1;
                }
                i += 1;
            }
        } else if c.is_ascii_digit() {
            let s = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let t: String = src[s..i].chars().filter(|&c| c != '_').collect();
            let v = if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
                u32::from_str_radix(h, 16).map(|v| v as i32).ok()
            } else {
                t.parse::<i64>().ok().filter(|v| *v <= u32::MAX as i64).map(|v| v as u32 as i32)
            };
            out.push((Tok::Int(v.ok_or_else(|| err(line, format!("bad number {t}")))?), line));
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let s = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            out.push((Tok::Ident(src[s..i].to_string()), line));
        } else if c == b'"' {
            i += 1;
            let mut s = String::new();
            loop {
                let Some(ch) = src[i..].chars().next() else { return Err(err(line, "unterminated string".into())) };
                i += ch.len_utf8();
                match ch {
                    '"' => break,
                    '\n' => return Err(err(line, "newline in string".into())),
                    '\\' => {
                        let e = src[i..].chars().next().ok_or_else(|| err(line, "unterminated string".into()))?;
                        i += e.len_utf8();
                        s.push(match e {
                            'n' => '\n',
                            't' => '\t',
                            '"' => '"',
                            '\\' => '\\',
                            _ => return Err(err(line, format!("unknown escape \\{e}"))),
                        });
                    }
                    _ => s.push(ch),
                }
            }
            out.push((Tok::Str(s), line));
        } else {
            let p = PUNCTS.iter().chain(PUNCTS1.iter()).find(|p| b[i..].starts_with(p.as_bytes()));
            match p {
                Some(p) => {
                    i += p.len();
                    out.push((Tok::Punct(p), line));
                }
                None => return Err(err(line, format!("unexpected character {:?}", src[i..].chars().next().unwrap_or('?')))),
            }
        }
    }
    out.push((Tok::Eof, line));
    Ok(out)
}

// ------------------------------------------------------------------ compiler

const KEYWORDS: [&str; 10] = ["let", "const", "fn", "if", "else", "while", "break", "continue", "return", "true"];

struct FnSig {
    idx: u16,
    arity: usize,
}

struct Compiler {
    toks: Vec<(Tok, u32)>,
    pos: usize,
    prog: Program,
    strings: BTreeMap<String, u32>,
    fns: BTreeMap<String, FnSig>,
    globals: BTreeMap<String, u16>,
    consts: BTreeMap<String, Value>,
    // current function
    scopes: Vec<Vec<(String, u16)>>,
    n_locals: usize,
    /// (continue target, break jump fixups) per enclosing loop.
    loops: Vec<(u32, Vec<usize>)>,
}

type R<T> = Result<T, CompileError>;

/// Compiles a script.
pub fn compile(src: &str) -> R<Program> {
    if src.len() > MAX_SOURCE {
        return Err(CompileError { line: 0, msg: format!("script larger than {MAX_SOURCE} bytes") });
    }
    let toks = lex(src)?;
    let mut c = Compiler {
        toks,
        pos: 0,
        prog: Program::default(),
        strings: BTreeMap::new(),
        fns: BTreeMap::new(),
        globals: BTreeMap::new(),
        consts: BTreeMap::new(),
        scopes: Vec::new(),
        n_locals: 0,
        loops: Vec::new(),
    };
    c.declare()?;
    c.program()?;
    Ok(c.prog)
}

impl Compiler {
    fn line(&self) -> u32 {
        self.toks[self.pos].1
    }
    fn err<T>(&self, msg: impl Into<String>) -> R<T> {
        Err(CompileError { line: self.line(), msg: msg.into() })
    }
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].0
    }
    fn peek_at(&self, n: usize) -> &Tok {
        &self.toks[(self.pos + n).min(self.toks.len() - 1)].0
    }
    fn next(&mut self) -> Tok {
        let t = self.toks[self.pos].0.clone();
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }
    fn is(&self, p: &str) -> bool {
        matches!(self.peek(), Tok::Punct(q) if *q == p)
    }
    fn is_kw(&self, k: &str) -> bool {
        matches!(self.peek(), Tok::Ident(s) if s == k)
    }
    fn eat(&mut self, p: &str) -> bool {
        if self.is(p) {
            self.next();
            true
        } else {
            false
        }
    }
    fn expect(&mut self, p: &str) -> R<()> {
        if self.eat(p) {
            Ok(())
        } else {
            self.err(format!("expected `{p}`, found {}", describe(self.peek())))
        }
    }
    fn ident(&mut self) -> R<String> {
        match self.peek().clone() {
            Tok::Ident(s) if !KEYWORDS.contains(&s.as_str()) && s != "false" => {
                self.next();
                Ok(s)
            }
            t => self.err(format!("expected a name, found {}", describe(&t))),
        }
    }
    fn emit(&mut self, op: Op) -> usize {
        self.prog.code.push(op);
        self.prog.code.len() - 1
    }
    fn here(&self) -> u32 {
        self.prog.code.len() as u32
    }
    fn patch(&mut self, at: usize, target: u32) {
        self.prog.code[at] = match self.prog.code[at] {
            Op::Jmp(_) => Op::Jmp(target),
            Op::Jz(_) => Op::Jz(target),
            Op::Jnz(_) => Op::Jnz(target),
            op => op,
        };
    }
    fn string(&mut self, s: String) -> u32 {
        let n = self.strings.len() as u32;
        *self.strings.entry(s.clone()).or_insert_with(|| {
            self.prog.strings.push(s);
            n
        })
    }

    /// Pre-pass: top-level `fn` signatures and `let` globals (forward references).
    fn declare(&mut self) -> R<()> {
        let mut depth = 0i32;
        let mut i = 0;
        let mut fn_count = 0u16;
        // function 0 is the global initialiser
        self.prog.funcs.push(Func { name: "<init>".into(), arity: 0, n_locals: 0, entry: 0 });
        fn_count += 1;
        while i < self.toks.len() {
            let (t, line) = (&self.toks[i].0, self.toks[i].1);
            let err = |msg: String| Err(CompileError { line, msg });
            match t {
                Tok::Punct("{") => depth += 1,
                Tok::Punct("}") => depth -= 1,
                Tok::Ident(k) if depth == 0 && k == "fn" => {
                    let Some((Tok::Ident(name), _)) = self.toks.get(i + 1) else { return err("expected a function name".into()) };
                    let name = name.clone();
                    let mut arity = 0;
                    let mut j = i + 3;
                    if !matches!(self.toks.get(j), Some((Tok::Punct(")"), _))) {
                        loop {
                            arity += 1;
                            j += 1;
                            match self.toks.get(j) {
                                Some((Tok::Punct(","), _)) => j += 1,
                                _ => break,
                            }
                        }
                    }
                    if self.fns.contains_key(&name) {
                        return err(format!("function {name} defined twice"));
                    }
                    if Native::by_name(&name).is_some() {
                        return err(format!("function {name} shadows a built-in function"));
                    }
                    if let Some(h) = HOOKS.iter().position(|(n, _)| *n == name) {
                        if HOOKS[h].1 != arity {
                            return err(format!("hook {name} takes {} parameters", HOOKS[h].1));
                        }
                        self.prog.hooks[h] = Some(fn_count);
                    }
                    if arity > MAX_PARAMS {
                        return err(format!("too many parameters (max {MAX_PARAMS})"));
                    }
                    self.fns.insert(name.clone(), FnSig { idx: fn_count, arity });
                    self.prog.funcs.push(Func { name, arity, n_locals: arity, entry: 0 });
                    fn_count = fn_count.checked_add(1).ok_or_else(|| CompileError { line, msg: "too many functions".into() })?;
                }
                Tok::Ident(k) if depth == 0 && k == "let" => {
                    let Some((Tok::Ident(name), _)) = self.toks.get(i + 1) else { return err("expected a variable name".into()) };
                    if self.globals.contains_key(name) {
                        return err(format!("global {name} declared twice"));
                    }
                    if self.globals.len() >= MAX_GLOBALS {
                        return err(format!("too many globals (max {MAX_GLOBALS})"));
                    }
                    self.globals.insert(name.clone(), self.prog.globals.len() as u16);
                    self.prog.globals.push(name.clone());
                }
                _ => {}
            }
            i += 1;
        }
        Ok(())
    }

    fn program(&mut self) -> R<()> {
        // Global initialisers first (function 0), then the functions.
        let mut fn_starts = Vec::new();
        self.prog.funcs[0].entry = 0;
        self.scopes = vec![Vec::new()];
        self.n_locals = 0;
        while *self.peek() != Tok::Eof {
            if self.is_kw("fn") {
                fn_starts.push(self.pos);
                self.skip_fn()?;
            } else if self.is_kw("let") {
                self.next();
                let name = self.ident()?;
                let g = self.globals[&name];
                if self.eat("=") {
                    self.expr()?;
                } else {
                    self.emit(Op::Int(0));
                }
                self.emit(Op::StoreG(g));
                self.expect(";")?;
            } else if self.is_kw("const") {
                self.next();
                let name = self.ident()?;
                self.expect("=")?;
                let neg = self.eat("-");
                let v = match self.next() {
                    Tok::Int(v) => Value::Int(if neg { v.wrapping_neg() } else { v }),
                    Tok::Str(s) if !neg => Value::Str(s),
                    t => return self.err(format!("const needs a number or string literal, found {}", describe(&t))),
                };
                if self.consts.insert(name.clone(), v).is_some() || self.globals.contains_key(&name) {
                    return self.err(format!("{name} declared twice"));
                }
                self.expect(";")?;
            } else {
                return self.err(format!("expected `fn`, `let` or `const` at top level, found {}", describe(self.peek())));
            }
        }
        self.emit(Op::Int(0));
        self.emit(Op::Ret);
        self.prog.funcs[0].n_locals = self.n_locals;
        for start in fn_starts {
            self.pos = start;
            self.function()?;
        }
        Ok(())
    }

    fn skip_fn(&mut self) -> R<()> {
        while !self.is("{") {
            if *self.peek() == Tok::Eof {
                return self.err("unexpected end of file");
            }
            self.next();
        }
        let mut depth = 0;
        loop {
            match self.next() {
                Tok::Punct("{") => depth += 1,
                Tok::Punct("}") => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                Tok::Eof => return self.err("unexpected end of file (missing `}`)"),
                _ => {}
            }
        }
    }

    fn function(&mut self) -> R<()> {
        self.next(); // fn
        let name = self.ident()?;
        let idx = self.fns[&name].idx as usize;
        self.expect("(")?;
        let mut params = Vec::new();
        if !self.is(")") {
            loop {
                let p = self.ident()?;
                if params.iter().any(|(q, _)| *q == p) {
                    return self.err(format!("parameter {p} given twice"));
                }
                params.push((p, params.len() as u16));
                if !self.eat(",") {
                    break;
                }
            }
        }
        self.expect(")")?;
        self.n_locals = params.len();
        self.scopes = vec![params];
        self.loops.clear();
        self.prog.funcs[idx].entry = self.here();
        self.block()?;
        self.emit(Op::Int(0));
        self.emit(Op::Ret);
        self.prog.funcs[idx].n_locals = self.n_locals;
        Ok(())
    }

    fn block(&mut self) -> R<()> {
        self.expect("{")?;
        self.scopes.push(Vec::new());
        while !self.eat("}") {
            if *self.peek() == Tok::Eof {
                return self.err("unexpected end of file (missing `}`)");
            }
            self.stmt()?;
        }
        self.scopes.pop();
        Ok(())
    }

    fn lookup_local(&self, name: &str) -> Option<u16> {
        self.scopes.iter().rev().flat_map(|s| s.iter().rev()).find(|(n, _)| n == name).map(|(_, i)| *i)
    }

    fn stmt(&mut self) -> R<()> {
        if self.is_kw("let") {
            self.next();
            let name = self.ident()?;
            if self.n_locals >= MAX_LOCALS {
                return self.err(format!("too many local variables (max {MAX_LOCALS})"));
            }
            if self.eat("=") {
                self.expr()?;
            } else {
                self.emit(Op::Int(0));
            }
            let slot = self.n_locals as u16;
            self.n_locals += 1;
            self.scopes.last_mut().expect("scope").push((name, slot));
            self.emit(Op::StoreL(slot));
            return self.expect(";");
        }
        if self.is_kw("if") {
            self.next();
            return self.if_rest();
        }
        if self.is_kw("while") {
            self.next();
            let top = self.here();
            self.expr()?;
            let jz = self.emit(Op::Jz(0));
            self.loops.push((top, Vec::new()));
            self.block()?;
            self.emit(Op::Jmp(top));
            let end = self.here();
            self.patch(jz, end);
            let (_, breaks) = self.loops.pop().expect("loop");
            for b in breaks {
                self.patch(b, end);
            }
            return Ok(());
        }
        if self.is_kw("break") || self.is_kw("continue") {
            let brk = self.is_kw("break");
            self.next();
            self.expect(";")?;
            let Some((top, _)) = self.loops.last() else { return self.err("break/continue outside a loop") };
            let top = *top;
            if brk {
                let j = self.emit(Op::Jmp(0));
                self.loops.last_mut().expect("loop").1.push(j);
            } else {
                self.emit(Op::Jmp(top));
            }
            return Ok(());
        }
        if self.is_kw("return") {
            self.next();
            if self.eat(";") {
                self.emit(Op::Int(0));
            } else {
                self.expr()?;
                self.expect(";")?;
            }
            self.emit(Op::Ret);
            return Ok(());
        }
        if self.is_kw("fn") || self.is_kw("const") {
            return self.err("functions and constants can only be declared at top level");
        }
        // assignment?
        if let (Tok::Ident(name), Tok::Punct(op)) = (self.peek().clone(), self.peek_at(1).clone()) {
            if ["=", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "<<=", ">>="].contains(&op) {
                self.next();
                self.next();
                let (load, store) = if let Some(l) = self.lookup_local(&name) {
                    (Op::LoadL(l), Op::StoreL(l))
                } else if let Some(&g) = self.globals.get(&name) {
                    (Op::LoadG(g), Op::StoreG(g))
                } else {
                    return self.err(format!("assignment to unknown variable {name}"));
                };
                if op != "=" {
                    self.emit(load);
                }
                self.expr()?;
                if op != "=" {
                    self.emit(binop(&op[..op.len() - 1]).expect("op"));
                }
                self.emit(store);
                return self.expect(";");
            }
        }
        self.expr()?;
        self.emit(Op::Pop);
        self.expect(";")
    }

    fn if_rest(&mut self) -> R<()> {
        self.expr()?;
        let jz = self.emit(Op::Jz(0));
        self.block()?;
        if self.is_kw("else") {
            self.next();
            let jend = self.emit(Op::Jmp(0));
            let h = self.here();
            self.patch(jz, h);
            if self.is_kw("if") {
                self.next();
                self.if_rest()?;
            } else {
                self.block()?;
            }
            let h = self.here();
            self.patch(jend, h);
        } else {
            let h = self.here();
            self.patch(jz, h);
        }
        Ok(())
    }

    // precedence climbing
    fn expr(&mut self) -> R<()> {
        self.or_expr()
    }

    fn or_expr(&mut self) -> R<()> {
        self.and_expr()?;
        if !self.is("||") {
            return Ok(());
        }
        let mut trues = Vec::new();
        while self.eat("||") {
            trues.push(self.emit(Op::Jnz(0)));
            self.and_expr()?;
        }
        trues.push(self.emit(Op::Jnz(0)));
        self.emit(Op::Int(0));
        let j = self.emit(Op::Jmp(0));
        let t = self.here();
        for x in trues {
            self.patch(x, t);
        }
        self.emit(Op::Int(1));
        let e = self.here();
        self.patch(j, e);
        Ok(())
    }

    fn and_expr(&mut self) -> R<()> {
        self.bin(0)?;
        if !self.is("&&") {
            return Ok(());
        }
        let mut falses = Vec::new();
        while self.eat("&&") {
            falses.push(self.emit(Op::Jz(0)));
            self.bin(0)?;
        }
        falses.push(self.emit(Op::Jz(0)));
        self.emit(Op::Int(1));
        let j = self.emit(Op::Jmp(0));
        let f = self.here();
        for x in falses {
            self.patch(x, f);
        }
        self.emit(Op::Int(0));
        let e = self.here();
        self.patch(j, e);
        Ok(())
    }

    /// Binary operator levels (lowest first): `|`, `^`, `&`, equality, comparison, shifts,
    /// additive, multiplicative.
    fn bin(&mut self, level: usize) -> R<()> {
        const LEVELS: [&[&str]; 8] = [&["|"], &["^"], &["&"], &["==", "!="], &["<", "<=", ">", ">="], &["<<", ">>"], &["+", "-"], &["*", "/", "%"]];
        if level == LEVELS.len() {
            return self.unary();
        }
        self.bin(level + 1)?;
        loop {
            let op = match self.peek() {
                Tok::Punct(p) if LEVELS[level].contains(p) => *p,
                _ => return Ok(()),
            };
            self.next();
            self.bin(level + 1)?;
            self.emit(binop(op).expect("op"));
        }
    }

    fn unary(&mut self) -> R<()> {
        if self.eat("-") {
            if let Tok::Int(v) = *self.peek() {
                self.next();
                self.emit(Op::Int(v.wrapping_neg()));
                return Ok(());
            }
            self.unary()?;
            self.emit(Op::Neg);
        } else if self.eat("!") {
            self.unary()?;
            self.emit(Op::Not);
        } else if self.eat("~") {
            self.unary()?;
            self.emit(Op::BitNot);
        } else {
            self.primary()?;
        }
        Ok(())
    }

    fn primary(&mut self) -> R<()> {
        match self.next() {
            Tok::Int(v) => {
                self.emit(Op::Int(v));
            }
            Tok::Str(s) => {
                let i = self.string(s);
                self.emit(Op::Str(i));
            }
            Tok::Punct("(") => {
                self.expr()?;
                self.expect(")")?;
            }
            Tok::Ident(name) if name == "true" || name == "false" => {
                self.emit(Op::Int((name == "true") as i32));
            }
            Tok::Ident(name) if !KEYWORDS.contains(&name.as_str()) => {
                if self.eat("(") {
                    let mut argc = 0;
                    if !self.is(")") {
                        loop {
                            self.expr()?;
                            argc += 1;
                            if !self.eat(",") {
                                break;
                            }
                        }
                    }
                    self.expect(")")?;
                    if let Some(sig) = self.fns.get(&name) {
                        if sig.arity != argc {
                            return self.err(format!("{name} takes {} arguments, {argc} given", sig.arity));
                        }
                        let idx = sig.idx;
                        self.emit(Op::Call(idx));
                    } else if let Some(n) = Native::by_name(&name) {
                        if n.arity() != argc {
                            return self.err(format!("{name} takes {} arguments, {argc} given", n.arity()));
                        }
                        self.emit(Op::Native(n as u16));
                    } else {
                        return self.err(format!("unknown function {name}"));
                    }
                } else if let Some(l) = self.lookup_local(&name) {
                    self.emit(Op::LoadL(l));
                } else if let Some(&g) = self.globals.get(&name) {
                    self.emit(Op::LoadG(g));
                } else if let Some(v) = self.consts.get(&name).cloned().or_else(|| builtin_const(&name).map(Value::Int)) {
                    match v {
                        Value::Int(i) => self.emit(Op::Int(i)),
                        Value::Str(s) => {
                            let i = self.string(s);
                            self.emit(Op::Str(i))
                        }
                    };
                } else {
                    return self.err(format!("unknown name {name}"));
                }
            }
            t => return self.err(format!("expected an expression, found {}", describe(&t))),
        }
        Ok(())
    }
}

fn binop(op: &str) -> Option<Op> {
    Some(match op {
        "+" => Op::Add,
        "-" => Op::Sub,
        "*" => Op::Mul,
        "/" => Op::Div,
        "%" => Op::Rem,
        "<<" => Op::Shl,
        ">>" => Op::Shr,
        "&" => Op::And,
        "|" => Op::Or,
        "^" => Op::Xor,
        "==" => Op::Eq,
        "!=" => Op::Ne,
        "<" => Op::Lt,
        "<=" => Op::Le,
        ">" => Op::Gt,
        ">=" => Op::Ge,
        _ => return None,
    })
}

fn describe(t: &Tok) -> String {
    match t {
        Tok::Int(v) => format!("number {v}"),
        Tok::Str(_) => "a string".into(),
        Tok::Ident(s) => format!("`{s}`"),
        Tok::Punct(p) => format!("`{p}`"),
        Tok::Eof => "end of file".into(),
    }
}
