//! Tokeniser, include stack and `%variable` table.
//!
//! Port of `Etc/parser.cpp`. The original kept all of this in globals
//! (`token`, `tokenType`, `FileStack`, the static look-ahead char in
//! `NextToken`); here it lives in [`Lexer`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// `EOF` as seen by the original (`char c = getc(...)`, so byte 0xFF is EOF too).
pub const EOF: i32 = -1;

/// Max include depth (`FILESTACKSIZE`).
pub const FILE_STACK_SIZE: usize = 10;

const WHITESPACE: &[u8] = b" \t\n\r";
const SYMBOLS: &[u8] = b"{}(),";

/// `TokenType` (`type.h`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenType {
    None,
    Number,
    String,
    Variable,
    Command,
    Symbol,
}

impl TokenType {
    /// `tokenNames[]` (`parser.cpp`).
    pub fn name(self) -> &'static str {
        match self {
            TokenType::None => "(null)",
            TokenType::Number => "number",
            TokenType::String => "string",
            TokenType::Variable => "variable",
            TokenType::Command => "command",
            TokenType::Symbol => "symbol",
        }
    }
}

/// A message printed by `Error()` (`file(line) : message`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub file: String,
    pub line: i64,
    pub message: String,
    /// The original printed everything through `Error()`; messages starting
    /// with "Warning" (and the "fuzzy" note) did not stop compilation.
    pub warning: bool,
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}({}) : {}", self.file, self.line, self.message)
    }
}

struct FileStackEntry {
    filename: String,
    data: Vec<u8>,
    pos: usize,
    line: i64,
}

/// Port of the `parser.cpp` globals and functions.
pub struct Lexer {
    files: Vec<FileStackEntry>,
    /// `static int c` in `NextToken` (look-ahead character).
    c: i32,
    /// `token[80]` (no length limit here).
    pub token: Vec<u8>,
    pub token_type: TokenType,
    /// `varLookup` (keys upper-cased like `Lookup`).
    vars: HashMap<Vec<u8>, Vec<u8>>,
    /// `includePath` (the original used the exe directory).
    pub include_path: Option<PathBuf>,
    /// In-memory files consulted before the file system (used by tests/tools).
    pub virtual_files: HashMap<String, Vec<u8>>,
    pub diagnostics: Vec<Diagnostic>,
    /// Name/line reported when no file is open.
    last_file: (String, i64),
}

impl Default for Lexer {
    fn default() -> Self {
        Self::new()
    }
}

fn is_variable_char(c: i32) -> bool {
    // `VARIABLESET.is_member(c)` indexes with (unsigned char)c.
    let b = (c & 0xFF) as u8;
    b.is_ascii_alphabetic() || b == b'_'
}

/// `strchr(set, c)`: also matches the NUL terminator when `c == 0`.
fn strchr(set: &[u8], c: i32) -> bool {
    let b = c as u8 as i32; // (char)c
    if c != b && c != EOF {
        return false;
    }
    if c == EOF {
        // (char)-1 == 0xFF: not in any of the ASCII sets.
        return false;
    }
    b == 0 || set.contains(&(b as u8))
}

/// MSVC `atoi`: optional sign and digits, wrapping on overflow.
pub fn c_atoi(s: &[u8]) -> i32 {
    let mut i = 0;
    while i < s.len() && matches!(s[i], b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c) {
        i += 1;
    }
    let mut neg = false;
    if i < s.len() && (s[i] == b'-' || s[i] == b'+') {
        neg = s[i] == b'-';
        i += 1;
    }
    let mut v: i32 = 0;
    while i < s.len() && s[i].is_ascii_digit() {
        v = v.wrapping_mul(10).wrapping_add((s[i] - b'0') as i32);
        i += 1;
    }
    if neg {
        v.wrapping_neg()
    } else {
        v
    }
}

/// `atof` restricted to what the tokenizer can produce (`[+-]digits[.digits]`).
pub fn c_atof(s: &[u8]) -> f64 {
    let mut i = 0;
    let mut out = String::new();
    if i < s.len() && (s[i] == b'-' || s[i] == b'+') {
        if s[i] == b'-' {
            out.push('-');
        }
        i += 1;
    }
    let mut digits = 0;
    out.push('0');
    while i < s.len() && s[i].is_ascii_digit() {
        out.push(s[i] as char);
        i += 1;
        digits += 1;
    }
    if i < s.len() && s[i] == b'.' {
        out.push('.');
        i += 1;
        while i < s.len() && s[i].is_ascii_digit() {
            out.push(s[i] as char);
            i += 1;
            digits += 1;
        }
        out.push('0');
    }
    if digits == 0 {
        return 0.0;
    }
    out.parse().unwrap_or(0.0)
}

impl Lexer {
    pub fn new() -> Self {
        Lexer {
            files: Vec::new(),
            c: b' ' as i32,
            token: Vec::new(),
            token_type: TokenType::None,
            vars: HashMap::new(),
            include_path: None,
            virtual_files: HashMap::new(),
            diagnostics: Vec::new(),
            last_file: (String::new(), 0),
        }
    }

    /// Port of `GetVariable`.
    pub fn get_variable(&self, key: &[u8]) -> Option<&[u8]> {
        self.vars.get(&key.to_ascii_uppercase()).map(|v| v.as_slice())
    }

    /// Port of `SetVariable`.
    pub fn set_variable(&mut self, key: &[u8], value: &[u8]) {
        self.vars.insert(key.to_ascii_uppercase(), value.to_vec());
    }

    /// Port of `CurrentFilename`.
    pub fn current_filename(&self) -> &str {
        self.files.last().map(|f| f.filename.as_str()).unwrap_or(&self.last_file.0)
    }

    /// Port of `CurrentLineNum`.
    pub fn current_line_num(&self) -> i64 {
        self.files.last().map(|f| f.line).unwrap_or(self.last_file.1)
    }

    /// Port of `Error`: records `file(line) : message`.
    pub fn error(&mut self, message: impl Into<String>) {
        let message = message.into();
        let warning = message.starts_with("Warning") || message == "fuzzy";
        self.diagnostics.push(Diagnostic {
            file: self.current_filename().to_string(),
            line: self.current_line_num(),
            message,
            warning,
        });
    }

    /// Number of files currently open (`currFile + 1`).
    pub fn depth(&self) -> usize {
        self.files.len()
    }

    /// Pushes an in-memory source as the next file (top-level compile input).
    pub fn push_source(&mut self, filename: &str, data: Vec<u8>) -> bool {
        if self.files.len() == FILE_STACK_SIZE {
            self.error("Too many open files");
            return false;
        }
        self.files.push(FileStackEntry { filename: filename.to_string(), data, pos: 0, line: 1 });
        true
    }

    fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        if let Some(d) = self.virtual_files.get(path) {
            return Some(d.clone());
        }
        std::fs::read(path).ok()
    }

    /// Port of `OpenFile`.
    ///
    /// Search order: the name as given, then (deviation: the original computed
    /// but never used this path) relative to the including file, then the
    /// include path.
    pub fn open_file(&mut self, filename: &str) -> bool {
        if self.files.len() == FILE_STACK_SIZE {
            self.error("Too many open files");
            return false;
        }
        let mut data = self.read_file(filename);
        if data.is_none() && !self.files.is_empty() {
            let cur = self.current_filename().to_string();
            let rel = Path::new(&cur).parent().map(|p| p.join(filename));
            if let Some(rel) = rel {
                data = self.read_file(&rel.to_string_lossy());
            }
            if data.is_none() {
                if let Some(inc) = &self.include_path {
                    data = self.read_file(&inc.join(filename).to_string_lossy());
                }
            }
            if data.is_none() {
                self.error(format!("Couldn't open '{filename}'"));
            }
        }
        match data {
            Some(data) => self.push_source(filename, data),
            None => false,
        }
    }

    /// Port of `CloseCurrFile`.
    pub fn close_curr_file(&mut self) {
        if let Some(f) = self.files.pop() {
            self.last_file = (f.filename, f.line);
        }
    }

    /// Port of `CloseAllFiles`.
    pub fn close_all_files(&mut self) {
        while !self.files.is_empty() {
            self.close_curr_file();
        }
    }

    fn getc(&mut self) -> i32 {
        let Some(f) = self.files.last_mut() else { return EOF };
        match f.data.get(f.pos) {
            Some(&b) => {
                f.pos += 1;
                if b == 0xFF {
                    EOF // `char c = getc()` makes 0xFF indistinguishable from EOF
                } else {
                    b as i32
                }
            }
            None => EOF,
        }
    }

    fn inc_line(&mut self) {
        if let Some(f) = self.files.last_mut() {
            f.line += 1;
        }
    }

    /// Port of `NextChar`: pops finished include files and strips `;` comments.
    fn next_char(&mut self) -> i32 {
        loop {
            let mut c = self.getc();
            if c == EOF && self.files.len() > 1 {
                self.close_curr_file();
                continue;
            } else if c == b'\n' as i32 {
                self.inc_line();
            } else if c == b';' as i32 {
                loop {
                    c = self.getc();
                    if c == EOF || c == b'\n' as i32 {
                        break;
                    }
                }
                self.inc_line();
                continue;
            }
            return c;
        }
    }

    /// Port of `NextToken`. Returns false at end of input or on a lexical error.
    pub fn next_token(&mut self) -> bool {
        self.token_type = TokenType::None;
        self.token.clear();

        while strchr(WHITESPACE, self.c) {
            self.c = self.next_char();
        }
        if self.c == EOF {
            self.c = b' ' as i32;
            return false;
        }

        let c = self.c;
        if strchr(SYMBOLS, c) {
            self.token.push(c as u8);
            self.token_type = TokenType::Symbol;
            self.c = self.next_char();
            return true;
        } else if c == b'%' as i32 {
            self.c = self.next_char();
            while self.c != EOF && is_variable_char(self.c) {
                self.token.push(self.c as u8);
                self.c = self.next_char();
            }
            self.token_type = TokenType::Variable;
            return true;
        } else if strchr(b"0123456789+-", c) {
            loop {
                self.token.push(self.c as u8);
                self.c = self.next_char();
                if !(self.c >= b'0' as i32 && self.c <= b'9' as i32) {
                    break;
                }
            }
            if self.c == b'.' as i32 {
                loop {
                    self.token.push(self.c as u8);
                    self.c = self.next_char();
                    if !(self.c >= b'0' as i32 && self.c <= b'9' as i32) {
                        break;
                    }
                }
            }
            self.token_type = TokenType::Number;
            return true;
        } else if c == b'"' as i32 {
            loop {
                self.c = self.next_char();
                if self.c == b'"' as i32 {
                    break;
                } else if self.c == EOF || self.c == b'\n' as i32 || self.c == b'\r' as i32 {
                    self.error("No closing quote");
                    return false;
                } else {
                    self.token.push(self.c as u8);
                }
            }
            self.token_type = TokenType::String;
            self.c = self.next_char();
            return true;
        } else if is_variable_char(c) && c != 0 {
            loop {
                self.token.push(self.c as u8);
                self.c = self.next_char();
                if !(self.c != EOF && is_variable_char(self.c)) {
                    break;
                }
            }
            self.token_type = TokenType::Command;
            return true;
        }

        self.error("Invalid character");
        self.c = b' ' as i32;
        false
    }

    /// Port of `GetStringToken`.
    pub fn get_string_token(&mut self) -> Option<Vec<u8>> {
        self.next_token();
        if self.token_type == TokenType::Variable {
            return self.get_variable(&self.token).map(|v| v.to_vec());
        } else if self.token_type != TokenType::String {
            return None;
        }
        Some(self.token.clone())
    }

    /// Port of `GetNumberToken`.
    pub fn get_number_token(&mut self) -> Option<f64> {
        self.next_token();
        if self.token_type == TokenType::Variable {
            let p = self.get_variable(&self.token)?;
            let mut i = 0;
            if p.first() == Some(&b'-') {
                i += 1;
            }
            while i < p.len() && p[i].is_ascii_digit() {
                i += 1;
            }
            if i < p.len() && p[i] == b'.' {
                i += 1;
                while i < p.len() && p[i].is_ascii_digit() {
                    i += 1;
                }
            }
            if i < p.len() {
                return None;
            }
            return Some(c_atof(p));
        }
        if self.token_type != TokenType::Number {
            return None;
        }
        Some(c_atof(&self.token))
    }

    /// Port of `GetIntegerToken`.
    pub fn get_integer_token(&mut self) -> Option<i32> {
        self.next_token();
        if self.token_type == TokenType::Variable {
            let p = self.get_variable(&self.token)?;
            let mut i = 0;
            if p.first() == Some(&b'-') {
                i += 1;
            }
            while i < p.len() && p[i].is_ascii_digit() {
                i += 1;
            }
            if i < p.len() {
                return None;
            }
            return Some(c_atoi(p));
        }
        if self.token_type != TokenType::Number {
            return None;
        }
        Some(c_atoi(&self.token))
    }

    /// The current token as text (lossy), for messages.
    pub fn token_str(&self) -> String {
        String::from_utf8_lossy(&self.token).into_owned()
    }

    /// First byte of the token (`token[0]`), 0 if empty.
    pub fn token0(&self) -> u8 {
        self.token.first().copied().unwrap_or(0)
    }
}
