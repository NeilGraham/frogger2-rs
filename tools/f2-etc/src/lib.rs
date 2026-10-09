//! `f2-etc`: the Frogger 2 event/trigger compiler (ETC) and a decompiler.
//!
//! * [`compiler`] is a close port of the original `teamSpirit/Etc` tool
//!   (`etc.cpp`, `parser.cpp`, `buffer.cpp`, `util.cpp`): ETC source → `.fev`.
//! * [`decompile`] turns a `.fev` back into ETC source that recompiles to the
//!   identical bytes.
//!
//! The command/trigger tables (`commands.ec`, `triggers.ec`) were not part of
//! the preserved source; `tables/` holds reconstructions from `codes.h` and
//! `script.c`, with script names chosen for this port. See `docs/ETC-LANGUAGE.md`.

#![forbid(unsafe_code)]

pub mod compiler;
pub mod decompile;
pub mod lexer;
pub mod table;

pub use compiler::{Compiled, Compiler, Options};
pub use decompile::{Decompiled, Fev};
pub use lexer::Diagnostic;

/// Compiles ETC source with the built-in tables.
pub fn compile(filename: &str, source: &[u8]) -> Result<Compiled, Vec<Diagnostic>> {
    Compiler::new(Options::default())?.compile_source(filename, source)
}

/// Decodes and decompiles a `.fev` image with the built-in tables.
pub fn decompile(data: &[u8], title: &str) -> Result<Decompiled, String> {
    let c = Compiler::new(Options::default()).map_err(|d| format!("{d:?}"))?;
    decompile_with(&c, data, title)
}

/// Decompiles using the tables loaded into `c`.
pub fn decompile_with(c: &Compiler, data: &[u8], title: &str) -> Result<Decompiled, String> {
    let fev = decompile::decode(data, &c.command_list, &c.trigger_list).map_err(|e| e.to_string())?;
    decompile::print(&fev, &c.command_list, &c.trigger_list, title)
}

/// Result of [`roundtrip`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Roundtrip {
    Identical,
    /// Recompiled output differs; first differing offset.
    Differs {
        offset: usize,
        original_len: usize,
        recompiled_len: usize,
    },
}

/// Decompiles `data`, recompiles the source and compares the bytes.
pub fn roundtrip(data: &[u8]) -> Result<(Decompiled, Roundtrip), String> {
    let dec = decompile(data, "roundtrip")?;
    let out = compile("roundtrip.etc", &dec.source).map_err(|d| d.iter().map(|x| x.to_string()).collect::<Vec<_>>().join("\n"))?;
    let r = if out.data == data {
        Roundtrip::Identical
    } else {
        let offset = data.iter().zip(&out.data).position(|(a, b)| a != b).unwrap_or(data.len().min(out.data.len()));
        Roundtrip::Differs { offset, original_len: data.len(), recompiled_len: out.data.len() }
    };
    Ok((dec, r))
}
