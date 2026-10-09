//! Game text `maps/language.fla`: FLA2-compressed tab-separated table.
//!
//! First row: `LANG_NAME \t Comments \t English \t Français \t Deutsch \t Italiano \t US`.
//! Each further row: string id, comment, then one column per language
//! (Latin-1). Parsed like `gameTextInit` (`PC/isllocal.c:32`): fields are
//! separated by TAB or line breaks and surrounding quotes are stripped.

use crate::read::{Error, Result};

/// Language column order in the file (`LANG_UK`, `LANG_F`, `LANG_D`, `LANG_IT`, `LANG_US`).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Language {
    Uk = 0,
    French = 1,
    German = 2,
    Italian = 3,
    Us = 4,
}

impl Language {
    pub const ALL: [Language; 5] = [Language::Uk, Language::French, Language::German, Language::Italian, Language::Us];
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug)]
pub struct StringTable {
    pub languages: Vec<String>,
    /// (id, comment, per-language text)
    pub rows: Vec<(String, String, Vec<String>)>,
}

fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

fn unquote(s: &str) -> String {
    let t = s.trim_matches(|c| c == '\r' || c == '\n');
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        t[1..t.len() - 1].replace("\"\"", "\"")
    } else {
        t.to_string()
    }
}

impl StringTable {
    /// Parses the decompressed table.
    pub fn parse(data: &[u8]) -> Result<StringTable> {
        let text = latin1(data);
        let mut lines = text.split("\r\n").filter(|l| !l.is_empty());
        let header = lines.next().ok_or_else(|| Error::new("language", "empty table"))?;
        let cols: Vec<&str> = header.split('\t').collect();
        if cols.len() < 3 {
            return Err(Error::new("language", "header has fewer than 3 columns"));
        }
        let languages: Vec<String> = cols[2..].iter().map(|s| unquote(s)).collect();
        let mut rows = Vec::new();
        for l in lines {
            let f: Vec<&str> = l.split('\t').collect();
            if f.is_empty() || f[0].trim().is_empty() {
                continue;
            }
            let texts = (0..languages.len()).map(|i| f.get(i + 2).map(|s| unquote(s)).unwrap_or_default()).collect();
            rows.push((unquote(f[0]), f.get(1).map(|s| unquote(s)).unwrap_or_default(), texts));
        }
        Ok(StringTable { languages, rows })
    }

    pub fn get(&self, id: &str, lang: Language) -> Option<&str> {
        self.rows.iter().find(|r| r.0 == id).and_then(|r| r.2.get(lang as usize)).map(|s| s.as_str())
    }

    /// Text by `STR_*` enum value (`GAMESTRING(n)`, `Common/lang.h`). Value 0 is
    /// `STR_LANGUAGE`, i.e. the header row (the language's own name); value
    /// `n > 0` is data row `n - 1`.
    pub fn by_index(&self, index: usize, lang: Language) -> Option<&str> {
        if index == 0 {
            return self.languages.get(lang as usize).map(|s| s.as_str());
        }
        self.rows.get(index - 1).and_then(|r| r.2.get(lang as usize)).map(|s| s.as_str())
    }

    /// `STR_*` id name of enum value `index` (`None` for 0, the header row).
    pub fn id_by_index(&self, index: usize) -> Option<&str> {
        index.checked_sub(1).and_then(|i| self.rows.get(i)).map(|r| r.0.as_str())
    }

    /// Number of `STR_*` values (`LANG_NUM_STRINGS`): data rows + the header.
    pub fn num_strings(&self) -> usize {
        self.rows.len() + 1
    }
}
