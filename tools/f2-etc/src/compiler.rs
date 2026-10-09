//! The ETC compiler proper. Port of `Etc/etc.cpp` and `Etc/buffer.cpp`.

use std::path::PathBuf;

use crate::lexer::{Diagnostic, Lexer, TokenType};
use crate::table::{self, ScriptTokenList, C_DEBUG, C_INCLUDE, C_ON, C_SET};
use crate::table::{PARAM_BLOCK, PARAM_BYTE, PARAM_FLOAT, PARAM_INT, PARAM_NONE, PARAM_STRING, PARAM_TRIGGER, PARAM_WORD};

/// `ETC_VERSION`.
pub const ETC_VERSION: u8 = 3;

/// Port of `Buffer::AddFloat`: `(int)(value * (float)0x10000)`.
///
/// Out-of-range results give `0x80000000` like the x87 `fistp` the original
/// compiled to.
pub fn float_to_fixed(value: f64) -> i32 {
    let f = value as f32;
    let p = f * 65536.0f32;
    if p.is_nan() || p >= 2147483648.0 || p < -2147483648.0 {
        i32::MIN
    } else {
        p as i32 // truncation toward zero
    }
}

/// `Buffer` (`buffer.cpp`): little-endian byte sink.
#[derive(Clone, Debug, Default)]
pub struct Buffer(pub Vec<u8>);

impl Buffer {
    pub fn clear(&mut self) {
        self.0.clear();
    }
    pub fn size(&self) -> usize {
        self.0.len()
    }
    pub fn add_uchar(&mut self, c: u8) {
        self.0.push(c);
    }
    pub fn add_word(&mut self, v: i32) {
        self.0.extend_from_slice(&(v as u16).to_le_bytes());
    }
    pub fn add_int(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    /// `AddString`: `u8` length (truncated `strlen`), then `len` bytes.
    pub fn add_string(&mut self, s: &[u8]) {
        let len = s.len() as u8;
        self.0.push(len);
        self.0.extend_from_slice(&s[..len as usize]);
    }
    pub fn add_float(&mut self, v: f64) {
        self.add_int(float_to_fixed(v));
    }
    pub fn append(&mut self, b: &Buffer) {
        self.0.extend_from_slice(&b.0);
    }
}

/// Options matching the original command-line switches.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// `-d`: emit `C_DEBUG` line-number records before each command.
    pub write_debug: bool,
    /// `-q`: suppress warnings.
    pub quiet: bool,
    /// Include search directory (the original used the exe's directory).
    pub include_path: Option<PathBuf>,
}

/// Result of a successful compile.
#[derive(Clone, Debug)]
pub struct Compiled {
    pub data: Vec<u8>,
    pub warnings: Vec<Diagnostic>,
    pub events: u32,
    pub triggers: u32,
}

/// Compiler state (the `etc.cpp` globals).
pub struct Compiler {
    pub lx: Lexer,
    pub trigger_list: ScriptTokenList,
    pub command_list: ScriptTokenList,
    pub opts: Options,
    debug_line_no: i64,
    pub events: u32,
    pub triggers: u32,
}

impl Compiler {
    /// `InitTables` with the built-in tables.
    pub fn new(opts: Options) -> Result<Self, Vec<Diagnostic>> {
        Self::with_tables(opts, table::DEFAULT_TRIGGERS_EC.as_bytes(), table::DEFAULT_COMMANDS_EC.as_bytes())
    }

    /// Port of `InitTables`: loads `triggers.ec` then `commands.ec` and adds
    /// the compiler commands.
    pub fn with_tables(opts: Options, triggers_ec: &[u8], commands_ec: &[u8]) -> Result<Self, Vec<Diagnostic>> {
        let mut lx = Lexer::new();
        lx.include_path = opts.include_path.clone();
        let mut trigger_list = ScriptTokenList::default();
        let mut command_list = ScriptTokenList::default();
        if !table::load_command_table(&mut lx, "triggers.ec", triggers_ec, &mut trigger_list) {
            return Err(lx.diagnostics);
        }
        if !table::load_command_table(&mut lx, "commands.ec", commands_ec, &mut command_list) {
            return Err(lx.diagnostics);
        }
        command_list.add_entry("Include", None, C_INCLUDE, &[0]);
        command_list.add_entry("Set", None, C_SET, &[0]);
        command_list.add_entry("", Some("C_DEBUG"), C_DEBUG, &[0]);
        Ok(Compiler { lx, trigger_list, command_list, opts, debug_line_no: 0, events: 0, triggers: 0 })
    }

    /// Port of `AddParamsToBuffer`.
    fn add_params_to_buffer(&mut self, b: &mut Buffer, params: &[u8]) -> bool {
        let lx = &mut self.lx;
        if !(lx.next_token() && lx.token_type == TokenType::Symbol && lx.token0() == b'(') {
            lx.error("Expecting '('");
            return false;
        }

        let mut ty = 0usize;
        loop {
            match params[ty] {
                PARAM_STRING => match self.lx.get_string_token() {
                    Some(c) => b.add_string(&c),
                    None => {
                        self.lx.error("Expecting string");
                        return false;
                    }
                },
                PARAM_INT => match self.lx.get_integer_token() {
                    Some(i) => b.add_int(i),
                    None => {
                        self.lx.error("Expecting integer");
                        return false;
                    }
                },
                PARAM_BYTE => match self.lx.get_integer_token() {
                    None => {
                        self.lx.error("Expecting integer");
                        return false;
                    }
                    Some(i) if !(0..=0xFF).contains(&i) => {
                        self.lx.error("Integer out of range (0-255)");
                        return false;
                    }
                    Some(i) => b.add_uchar(i as u8),
                },
                PARAM_WORD => match self.lx.get_integer_token() {
                    None => {
                        self.lx.error("Expecting integer");
                        return false;
                    }
                    Some(i) if !(-0xFFFF..=0xFFFF).contains(&i) => {
                        self.lx.error("Integer out of range (-65535 to 65535)");
                        return false;
                    }
                    Some(i) => b.add_word(i),
                },
                PARAM_FLOAT => match self.lx.get_number_token() {
                    Some(v) => b.add_float(v),
                    None => {
                        self.lx.error("Expecting number");
                        return false;
                    }
                },
                PARAM_BLOCK => {
                    if !self.add_block(b) {
                        return false;
                    }
                }
                PARAM_TRIGGER => {
                    if !self.add_trigger(b) {
                        return false;
                    }
                }
                PARAM_NONE => {
                    let lx = &mut self.lx;
                    if !(lx.next_token() && lx.token_type == TokenType::Symbol && lx.token0() == b')') {
                        lx.error("Expecting ')'");
                        return false;
                    }
                    return true;
                }
                _ => {}
            }

            ty += 1;
            let lx = &mut self.lx;
            if params[ty] != PARAM_NONE {
                if !(lx.next_token() && lx.token_type == TokenType::Symbol && lx.token0() == b',') {
                    lx.error("Expecting ','");
                    return false;
                }
            } else {
                if !(lx.next_token() && lx.token_type == TokenType::Symbol && lx.token0() == b')') {
                    lx.error("Expecting ')'");
                    return false;
                }
                return true;
            }
        }
    }

    /// Port of `AddEvent`: compiles the command whose name is the current token.
    fn add_event(&mut self, buffer: &mut Buffer) -> bool {
        self.debug_line_no = self.lx.current_line_num();

        let Some(info) = self.command_list.get_entry(&self.lx.token).cloned() else {
            let msg = format!("'{}' is not a valid event", self.lx.token_str());
            self.lx.error(msg);
            return false;
        };

        match info.token {
            C_INCLUDE => {
                let Some(filename) = self.lx.get_string_token() else {
                    self.lx.error("Expected filename");
                    return false;
                };
                let filename = String::from_utf8_lossy(&filename).into_owned();
                if !self.lx.open_file(&filename) {
                    return false;
                }
                self.lx.next_token();
                self.add_event(buffer)
            }
            C_SET => {
                self.lx.next_token();
                if self.lx.token_type != TokenType::Variable {
                    self.lx.error("Expected variable name");
                    return false;
                }
                let name = self.lx.token.clone();
                self.lx.next_token();
                let value = self.lx.token.clone();
                self.lx.set_variable(&name, &value);
                true
            }
            C_ON => {
                buffer.add_uchar(C_ON as u8);
                if !self.add_trigger(buffer) {
                    return false;
                }
                match self.lx.get_integer_token() {
                    Some(v) if (0..=255).contains(&v) => buffer.add_uchar(v as u8),
                    _ => {
                        self.lx.error("Expecting trigger flags");
                        return false;
                    }
                }
                self.add_block(buffer)
            }
            _ => {
                buffer.add_uchar(info.token as u8);
                if !self.add_params_to_buffer(buffer, &info.params) {
                    return false;
                }
                self.events += 1;
                true
            }
        }
    }

    /// Port of `WriteDebug`.
    fn write_debug(&self, buffer: &mut Buffer) {
        buffer.add_uchar(C_DEBUG as u8);
        buffer.add_word(0); // placeholder
        buffer.add_word(self.debug_line_no as i32);
    }

    /// Port of `AddBlock`: a single command or `{ commands }`, written as
    /// `i32 size, i16 count, commands`.
    fn add_block(&mut self, buffer: &mut Buffer) -> bool {
        let mut b = Buffer::default();
        let mut bb = Buffer::default();
        let mut e: i32 = 0;

        if !self.lx.next_token() {
            self.lx.error("Expected event");
            return false;
        }

        if self.lx.token_type != TokenType::Symbol {
            if !self.add_event(&mut bb) {
                return false;
            }
            if bb.size() == 0 && !self.opts.quiet {
                self.lx.error("Warning: Command does nothing in interpreter");
            }
            e = 1;
            if self.opts.write_debug {
                self.write_debug(&mut b);
                e = 2;
            }
            b.append(&bb);
        } else {
            // BUG(original): `else if (token[0] = '{')` is an assignment, so any
            // symbol opens a block, not just '{'. Kept for byte compatibility.
            loop {
                self.lx.next_token();
                if self.lx.token_type == TokenType::Symbol && self.lx.token0() == b'}' {
                    break;
                }
                bb.clear();
                if !self.add_event(&mut bb) {
                    return false;
                }
                if bb.size() != 0 {
                    if self.opts.write_debug {
                        self.write_debug(&mut b);
                        e += 1;
                    }
                    b.append(&bb);
                    e += 1;
                } else {
                    self.lx.error("fuzzy");
                }
            }
            if e == 0 {
                self.lx.error("Warning: Block contains no commands");
            }
        }

        buffer.add_int(b.size() as i32 + 2); // size of [block + number of events]
        buffer.add_word(e); // number of events
        buffer.append(&b);
        true
    }

    /// Port of `AddTrigger`.
    fn add_trigger(&mut self, output: &mut Buffer) -> bool {
        self.lx.next_token();

        if self.lx.token_type != TokenType::Command {
            let msg = format!("Found {} when expecting a trigger name", self.lx.token_type.name());
            self.lx.error(msg);
            return false;
        }

        let Some(info) = self.trigger_list.get_entry(&self.lx.token).cloned() else {
            let msg = format!("'{}' is not a valid trigger", self.lx.token_str());
            self.lx.error(msg);
            return false;
        };

        output.add_uchar(info.token as u8);
        if !self.add_params_to_buffer(output, &info.params) {
            return false;
        }
        self.triggers += 1;
        true
    }

    /// Port of `compile` (minus the file writing): compiles one source file
    /// and returns the complete `.fev` image.
    ///
    /// Deviation: any non-warning `Error()` fails the compile. The original
    /// only failed on some of them (a lexical error at top level silently
    /// ended the file and still saved it).
    pub fn compile_source(&mut self, filename: &str, source: &[u8]) -> Result<Compiled, Vec<Diagnostic>> {
        self.lx.close_all_files();
        self.lx.diagnostics.clear();
        self.events = 0;
        self.triggers = 0;

        let mut buffer = Buffer::default();
        let mut b = Buffer::default();
        let mut e: i32 = 0;

        if !self.lx.push_source(filename, source.to_vec()) {
            return Err(std::mem::take(&mut self.lx.diagnostics));
        }

        let mut ok = true;
        while self.lx.next_token() {
            b.clear();
            if !self.add_event(&mut b) {
                ok = false;
                break;
            }
            if b.size() != 0 {
                if self.opts.write_debug {
                    self.write_debug(&mut buffer);
                    e += 1;
                }
                buffer.append(&b);
                e += 1;
            }
        }
        self.lx.close_all_files();

        let diags = std::mem::take(&mut self.lx.diagnostics);
        if !ok || diags.iter().any(|d| !d.warning) {
            return Err(diags);
        }

        let mut header = Buffer::default();
        header.add_uchar(ETC_VERSION);
        header.add_int(buffer.size() as i32 + 2); // size of entire script (outermost block)
        header.add_word(e); // number of events in outer block
        header.append(&buffer);

        let warnings = if self.opts.quiet { Vec::new() } else { diags };
        Ok(Compiled { data: header.0, warnings, events: self.events, triggers: self.triggers })
    }

    /// Port of `OutputHeader`: the `codes.h` text. `created` is the
    /// `asctime()` string (including its trailing newline).
    pub fn output_header(&self, created: &str) -> String {
        let mut s = String::new();
        s.push_str("#ifndef _CODES_H_INCLUDED\n#define _CODES_H_INCLUDED\n\n/* codes.h automatically generated by ETC\n");
        s.push_str(&format!("   Created: {created}\n"));
        s.push_str("----------------------------------------------------------------*/\n\n/* Commands */\n\n");
        self.command_list.output_defines(&mut s);
        s.push_str("\n/* Triggers */\n\n");
        self.trigger_list.output_defines(&mut s);
        s.push_str("#endif\n");
        s
    }
}

/// `asctime()` of a Unix time (UTC): `"Thu May 11 19:09:59 2000\n"`.
pub fn asctime_utc(secs: i64) -> String {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (h, m, s) = (rem / 3600, rem % 3600 / 60, rem % 60);
    let wday = (days + 4).rem_euclid(7); // 1970-01-01 was a Thursday
                                         // Civil-from-days (Howard Hinnant).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(mo <= 2);
    const WD: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MN: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{} {} {:2} {:02}:{:02}:{:02} {}\n", WD[wday as usize], MN[(mo - 1) as usize], d, h, m, s, y)
}
