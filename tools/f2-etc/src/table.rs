//! Command/trigger tables (`commands.ec`, `triggers.ec`).
//!
//! Port of `ScriptTokenList` (`Etc/util.cpp`) and `LoadCommandTable`
//! (`Etc/etc.cpp`). The original tables were not preserved; `tables/*.ec`
//! are reconstructions from `Common/codes.h` and `Common/script.c`.

use std::collections::HashMap;

use crate::lexer::{Lexer, TokenType};

/// `ParamTypeEnum` (`type.h`).
pub const PARAM_NONE: u8 = 0;
pub const PARAM_BYTE: u8 = 1;
pub const PARAM_WORD: u8 = 2;
pub const PARAM_INT: u8 = 3;
pub const PARAM_FLOAT: u8 = 4;
pub const PARAM_STRING: u8 = 5;
pub const PARAM_TRIGGER: u8 = 6;
pub const PARAM_BLOCK: u8 = 7;
pub const NUM_PARAM_TYPE: i32 = 8;

/// Compiler pseudo-commands (`etc.h`).
pub const C_ON: i32 = 0x40;
pub const C_DEBUG: i32 = 0xFF;
pub const C_INCLUDE: i32 = 0x100;
pub const C_SET: i32 = 0x101;

/// Built-in default tables.
pub const DEFAULT_TRIGGERS_EC: &str = include_str!("../tables/triggers.ec");
pub const DEFAULT_COMMANDS_EC: &str = include_str!("../tables/commands.ec");

/// `ScriptToken`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptToken {
    pub name: String,
    pub define: Option<String>,
    pub token: i32,
    /// Parameter types, terminated by `PARAM_NONE` (always present).
    pub params: Vec<u8>,
}

/// `ScriptTokenList`: entries in insertion order plus a case-insensitive lookup.
#[derive(Clone, Debug, Default)]
pub struct ScriptTokenList {
    pub entries: Vec<ScriptToken>,
    lookup: HashMap<String, usize>,
}

impl ScriptTokenList {
    /// Port of `ScriptTokenList::AddEntry`; duplicate names (case-insensitive) are rejected.
    pub fn add_entry(&mut self, name: &str, define: Option<&str>, token: i32, params: &[u8]) -> bool {
        let key = name.to_ascii_uppercase();
        if self.lookup.contains_key(&key) {
            return false;
        }
        let mut p = params.to_vec();
        p.push(PARAM_NONE);
        self.lookup.insert(key, self.entries.len());
        self.entries.push(ScriptToken { name: name.to_string(), define: define.map(str::to_string), token, params: p });
        true
    }

    /// Port of `ScriptTokenList::GetEntry` (case-insensitive).
    pub fn get_entry(&self, name: &[u8]) -> Option<&ScriptToken> {
        let key = String::from_utf8_lossy(name).to_ascii_uppercase();
        self.lookup.get(&key).map(|&i| &self.entries[i])
    }

    /// First entry (in table order) with the given opcode and a usable name.
    pub fn by_token(&self, token: i32) -> Option<&ScriptToken> {
        self.entries.iter().find(|e| e.token == token && !e.name.is_empty())
    }

    /// Port of `ScriptTokenList::OutputDefines` (the original list was a
    /// prepend-linked list, so defines come out in reverse table order).
    pub fn output_defines(&self, out: &mut String) {
        for t in self.entries.iter().rev() {
            if let Some(d) = &t.define {
                out.push_str(&format!("#define {:<24} {}\n", d, t.token));
            }
        }
    }
}

/// Port of `LoadCommandTable`. `source` is the table file text.
pub fn load_command_table(lx: &mut Lexer, filename: &str, source: &[u8], list: &mut ScriptTokenList) -> bool {
    let mut err = false;

    if !lx.push_source(filename, source.to_vec()) {
        return false;
    }

    lx.set_variable(b"p_void", b"0");
    lx.set_variable(b"p_byte", b"1");
    lx.set_variable(b"p_word", b"2");
    lx.set_variable(b"p_integer", b"3");
    lx.set_variable(b"p_float", b"4");
    lx.set_variable(b"p_string", b"5");
    lx.set_variable(b"p_trigger", b"6");
    lx.set_variable(b"p_block", b"7");

    while !err {
        let mut params: Vec<u8> = Vec::new();

        let Some(token_val) = lx.get_integer_token() else {
            if lx.token_type == TokenType::None {
                break; // end of file
            }
            lx.error("Expected token number");
            err = true;
            break;
        };

        if !(0..=255).contains(&token_val) {
            lx.error("Tokens must be 0-255");
            err = true;
            break;
        }

        lx.next_token();
        if lx.token_type != TokenType::Command {
            lx.error("Expected C/C++ #define name");
            err = true;
            break;
        }
        let define = lx.token_str();

        lx.next_token();
        if lx.token_type != TokenType::Command {
            lx.error("Expected token name");
            err = true;
            break;
        }
        let name = lx.token_str();

        if !(lx.next_token() && lx.token_type == TokenType::Symbol && lx.token0() == b'(') {
            lx.error("Expecting '('");
            err = true;
            break;
        }

        while !err {
            let Some(i) = lx.get_integer_token() else {
                lx.error("Expecting param type");
                err = true;
                break;
            };
            if !(0..NUM_PARAM_TYPE).contains(&i) {
                lx.error("Param type out of range");
                err = true;
                break;
            }
            params.push(i as u8);

            if !lx.next_token() || lx.token_type == TokenType::Symbol {
                if lx.token0() == b')' {
                    list.add_entry(&name, Some(&define), token_val, &params);
                    break;
                } else if lx.token0() != b',' {
                    lx.error("Invalid Symbol");
                    err = true;
                }
            }
        }
    }

    lx.close_curr_file();
    !err
}
