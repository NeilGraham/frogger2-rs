//! Game text overrides merged over the retail `maps/language.fla` table.
//!
//! - `text/language.tsv`: same columns as retail / `f2-export` output:
//!   `id <TAB> comment <TAB> <language>…`. Language columns are matched to
//!   the base table by name (case-insensitive); unknown names add a new
//!   language column. Rows are matched by string id; empty cells keep the
//!   base text; unknown ids are appended (existing `STR_*` indices never shift).
//! - `text/<lang>.toml`: `name = "Español"` (optional display name for a new
//!   language) and a `[strings]` table `STR_ID = "text"`. `<lang>` is a base
//!   language name or alias (`uk`/`en`/`english`, `fr`/`french`, `de`/`german`,
//!   `it`/`italian`, `us`), otherwise a new language.
//!
//! Cells of new languages that a mod leaves empty fall back to the first
//! (UK English) column so the game never shows blank text.

use std::collections::BTreeMap;

use f2_formats::lang::StringTable;
use serde::Deserialize;

use crate::Error;

fn unquote(s: &str) -> String {
    let t = s.trim_matches(|c| c == '\r' || c == '\n');
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        t[1..t.len() - 1].replace("\"\"", "\"")
    } else {
        t.to_string()
    }
}

/// Column index of `name` in `t.languages`, adding a new language if needed.
fn lang_column(t: &mut StringTable, name: &str, added: &mut Vec<usize>) -> usize {
    let alias = match name.to_ascii_lowercase().as_str() {
        "uk" | "en" | "english" => Some(0),
        "fr" | "french" | "francais" | "français" => Some(1),
        "de" | "german" | "deutsch" => Some(2),
        "it" | "italian" | "italiano" => Some(3),
        "us" => Some(4),
        _ => None,
    };
    if let Some(i) = t.languages.iter().position(|l| l.to_lowercase() == name.to_lowercase()) {
        return i;
    }
    if let Some(i) = alias.filter(|&i| i < t.languages.len()) {
        return i;
    }
    t.languages.push(name.to_string());
    for r in &mut t.rows {
        r.2.push(String::new());
    }
    added.push(t.languages.len() - 1);
    t.languages.len() - 1
}

fn set(t: &mut StringTable, id: &str, col: usize, text: &str) {
    if text.is_empty() {
        return;
    }
    match t.rows.iter_mut().find(|r| r.0 == id) {
        Some(r) => r.2[col] = text.to_string(),
        None => {
            let mut texts = vec![String::new(); t.languages.len()];
            texts[col] = text.to_string();
            t.rows.push((id.to_string(), String::new(), texts));
        }
    }
}

/// Fills empty cells of `cols` from column 0.
fn fill_fallback(t: &mut StringTable, cols: &[usize]) {
    for r in &mut t.rows {
        for &c in cols {
            if r.2[c].is_empty() {
                r.2[c] = r.2.first().cloned().unwrap_or_default();
            }
        }
    }
}

/// Merges a `language.tsv` override into `t`.
pub fn merge_tsv(t: &mut StringTable, text: &str) -> Result<(), Error> {
    let mut lines = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).filter(|l| !l.trim().is_empty());
    let header = lines.next().ok_or_else(|| Error::new("language.tsv: empty"))?;
    let cols: Vec<String> = header.split('\t').map(unquote).collect();
    if cols.len() < 3 {
        return Err(Error::new("language.tsv: header needs id, comment and at least one language column"));
    }
    let mut added = Vec::new();
    let map: Vec<usize> = cols[2..].iter().map(|n| lang_column(t, n, &mut added)).collect();
    for l in lines {
        let f: Vec<String> = l.split('\t').map(unquote).collect();
        let id = f[0].trim();
        if id.is_empty() {
            continue;
        }
        for (k, &col) in map.iter().enumerate() {
            if let Some(s) = f.get(k + 2) {
                set(t, id, col, s);
            }
        }
        if let Some(c) = f.get(1).filter(|c| !c.is_empty()) {
            if let Some(r) = t.rows.iter_mut().find(|r| r.0 == id) {
                r.1 = c.clone();
            }
        }
    }
    fill_fallback(t, &added);
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LangToml {
    name: Option<String>,
    #[serde(default)]
    strings: BTreeMap<String, String>,
}

/// Merges a `text/<lang>.toml` override into `t`.
pub fn merge_toml(t: &mut StringTable, lang: &str, text: &str) -> Result<(), Error> {
    let f: LangToml = toml::from_str(text).map_err(|e| Error::new(format!("{lang}.toml: {e}")))?;
    let mut added = Vec::new();
    let col = match lang_column_lookup(t, lang) {
        Some(c) => c,
        None => lang_column(t, f.name.as_deref().unwrap_or(lang), &mut added),
    };
    for (id, s) in &f.strings {
        set(t, id, col, s);
    }
    fill_fallback(t, &added);
    Ok(())
}

fn lang_column_lookup(t: &StringTable, lang: &str) -> Option<usize> {
    let mut probe = t.clone();
    let mut added = Vec::new();
    let c = lang_column(&mut probe, lang, &mut added);
    added.is_empty().then_some(c)
}

/// Characters the game's Latin-1 font can't show, for validation warnings.
pub fn non_latin1(t: &StringTable) -> Vec<(String, char)> {
    let mut out = Vec::new();
    for r in &t.rows {
        if let Some(c) = r.2.iter().flat_map(|s| s.chars()).find(|&c| c as u32 > 0xff) {
            out.push((r.0.clone(), c));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> StringTable {
        StringTable {
            languages: vec!["English".into(), "Français".into(), "Deutsch".into(), "Italiano".into(), "US".into()],
            rows: vec![("STR_A".into(), "c".into(), vec!["a".into(), "fa".into(), "da".into(), "ia".into(), "ua".into()])],
        }
    }

    #[test]
    fn tsv_merges_and_adds_language() {
        let mut t = base();
        merge_tsv(&mut t, "id\tcomment\tenglish\tEspañol\r\nSTR_A\t\tA!\tes-a\nSTR_NEW\tnew\tnew-en\t\n").unwrap();
        assert_eq!(t.languages.len(), 6);
        assert_eq!(t.rows[0].2[0], "A!");
        assert_eq!(t.rows[0].2[1], "fa");
        assert_eq!(t.rows[0].2[5], "es-a");
        assert_eq!(t.rows[1].0, "STR_NEW");
        assert_eq!(t.rows[1].2[5], "new-en");
        assert_eq!(t.rows[0].1, "c");
    }

    #[test]
    fn toml_by_alias_and_new() {
        let mut t = base();
        merge_toml(&mut t, "fr", "[strings]\nSTR_A = \"bonjour\"\n").unwrap();
        assert_eq!(t.rows[0].2[1], "bonjour");
        merge_toml(&mut t, "es", "name = \"Español\"\n[strings]\nSTR_A = \"hola\"\n").unwrap();
        assert_eq!(t.languages[5], "Español");
        assert_eq!(t.rows[0].2[5], "hola");
        assert!(merge_toml(&mut t, "x", "bogus = 1").is_err());
        assert!(non_latin1(&t).is_empty());
    }
}
