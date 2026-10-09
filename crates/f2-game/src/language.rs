//! Game text: port of `Common/language.c` (`DIKStrings`, key names for the PC key-remap
//! screen), `PC/isllocal.c` (`gameTextInit`: the localised string table) and the
//! `GAMESTRING(n)` accessor of `Common/lang.h`.
//!
//! The PC game loads `MAPS\LANGUAGE.FLA` (FLA2-compressed tab-separated table, parsed by
//! [`f2_formats::lang::StringTable`]). `gameTextInit` walks `numStrings` rows, skipping the id
//! and comment columns, and keeps the column of the current language.

use f2_formats::lang::{Language, StringTable};

use crate::assets::DataSource;
use crate::effects::Effect;
use crate::Game;

// `enum { LANG_UK, LANG_F, LANG_D, LANG_IT, LANG_US, LANG_NUMLANGS }` (`lang.h:16`).
pub const LANG_UK: i32 = 0;
pub const LANG_F: i32 = 1;
pub const LANG_D: i32 = 2;
pub const LANG_IT: i32 = 3;
pub const LANG_US: i32 = 4;
pub const LANG_NUMLANGS: usize = 5;
/// `LANG_DEFAULT` (`lang.h:26`): `LANG_US` for the NTSC (USA) build, `LANG_UK` for PAL.
pub const LANG_DEFAULT: i32 = LANG_US;

/// `LANG_NUM_STRINGS` of the PC build (`lang.h:578`, with the `PC_VERSION` entries).
pub const LANG_NUM_STRINGS: usize = 540;

/// Text returned by `GAMESTRING(n)` for an id outside `1..LANG_NUM_STRINGS` (`lang.h:582`).
pub const OUT_OF_RANGE_STRING: &str = "out of range string";

/// Path of the string table the PC version loads.
pub const LANGUAGE_FILE: &str = "MAPS\\LANGUAGE.FLA";

/// `STR_*` ids (`enum GAMESTRINGS`, `lang.h:32`) used by the ported code.
pub mod str_id {
    pub const STR_LANGUAGE: i32 = 0;
    pub const STR_GAMEOVER: i32 = 69;
    pub const STR_RESTARTLEVEL: i32 = 70;
    pub const STR_QUIT: i32 = 71;
    pub const STR_MULTI_PLAYERWINS: i32 = 81;
    pub const STR_MULTI_WINMATCH: i32 = 82;
    pub const STR_YOUTOOKTIMEMIN: i32 = 90;
    pub const STR_PRESS_X_TO_CONTINUE: i32 = 101;
    pub const STR_GOT_ALL_COINS: i32 = 143;
    pub const STR_MISSED_COINS: i32 = 144;
    pub const STR_NO_BONUS: i32 = 145;
    pub const STR_MULTI_DRAW: i32 = 149;
    pub const STR_ENTER_NAME: i32 = 151;
    pub const STR_NEW_BEST_TIME: i32 = 152;
    pub const STR_OPENED_EXTRA_1: i32 = 155;
    pub const STR_TRAINING_COMPLETE: i32 = 220;
    pub const STR_RECORD: i32 = 223;
    pub const STR_CHAR_NAME_1: i32 = 236;
    pub const STR_THE_END: i32 = 250;
}

/// `gameTextStr` / `gameTextLang` (`PC/isllocal.c:19`, `Common/layout.c:98`).
#[derive(Clone, Debug)]
pub struct LanguageState {
    /// `gameTextLang`.
    pub game_text_lang: i32,
    /// `gameTextStr[]` for the current language (index = `STR_*`); empty until loaded.
    pub game_text_str: Vec<String>,
}

impl Default for LanguageState {
    fn default() -> Self {
        LanguageState { game_text_lang: LANG_DEFAULT, game_text_str: Vec::new() }
    }
}

fn language_of(lang: i32) -> Language {
    match lang {
        LANG_F => Language::French,
        LANG_D => Language::German,
        LANG_IT => Language::Italian,
        LANG_US => Language::Us,
        _ => Language::Uk,
    }
}

impl LanguageState {
    /// Port of `gameTextInit(fName, numStrings, numLang, currLang)` (PC/isllocal.c:32) on an
    /// already parsed table: string `n` is the current language's column of row `n` (row 0
    /// being the header, i.e. the language's own name). Rows missing from the file read as "".
    pub fn game_text_init(&mut self, table: &StringTable, num_strings: usize, curr_lang: i32) {
        self.game_text_lang = curr_lang;
        let lang = language_of(curr_lang);
        self.game_text_str = (0..num_strings).map(|n| table.by_index(n, lang).unwrap_or("").to_string()).collect();
    }

    /// `GAMESTRING(n)` (`lang.h:582`).
    pub fn gamestring(&self, n: i32) -> &str {
        if n > 0 && (n as usize) < LANG_NUM_STRINGS {
            self.game_text_str.get(n as usize).map_or("", |s| s.as_str())
        } else {
            OUT_OF_RANGE_STRING
        }
    }

    /// `gameTextDestroy()` (PC/isllocal.c:99).
    pub fn game_text_destroy(&mut self) {
        self.game_text_str.clear();
    }
}

/// `DIKStrings[key][gameTextLang]` (`Common/language.c:16`): name of a DirectInput key.
/// Keys past the initialised part of the table are `NULL` in the original (`None`).
pub fn dik_string(key: u8, lang: i32) -> Option<&'static str> {
    DIK_STRINGS.get(key as usize).and_then(|r| r.get(lang as usize)).copied()
}

impl Game {
    /// Loads the game text: `gameTextInit("MAPS\LANGUAGE.FLA", LANG_NUM_STRINGS,
    /// LANG_NUMLANGS, lang)`. Logs and leaves the text empty if the file is missing.
    pub fn load_game_text(&mut self, data: &dyn DataSource, lang: i32) {
        let table = data
            .read(LANGUAGE_FILE)
            .ok_or_else(|| format!("missing {LANGUAGE_FILE}"))
            .and_then(|b| f2_formats::fla::maybe_decompress(b).map_err(|e| e.to_string()))
            .and_then(|b| StringTable::parse(&b).map_err(|e| e.to_string()));
        match table {
            Ok(t) => self.language.game_text_init(&t, LANG_NUM_STRINGS, lang),
            Err(e) => {
                self.language.game_text_lang = lang;
                self.effects.push(Effect::Log { text: format!("gameTextInit: {e}") });
            }
        }
    }

    /// `GAMESTRING(n)` as an owned string.
    pub fn gamestring(&self, n: i32) -> String {
        self.language.gamestring(n).to_string()
    }
}

/// An argument of [`c_sprintf`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FmtArg {
    Int(i32),
    Str(String),
}

/// Minimal `sprintf` for the game's format strings: `%d`/`%i`/`%u`, `%s`, `%c`, `0`/`-`
/// flags and width (`%02d`), and `%%`. Missing arguments print nothing.
pub fn c_sprintf(fmt: &str, args: &[FmtArg]) -> String {
    let mut out = String::new();
    let mut it = fmt.chars().peekable();
    let mut ai = 0;
    while let Some(c) = it.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        if it.peek() == Some(&'%') {
            it.next();
            out.push('%');
            continue;
        }
        let mut zero = false;
        let mut left = false;
        let mut width = 0usize;
        while let Some(&f) = it.peek() {
            match f {
                '0' if width == 0 => zero = true,
                '-' => left = true,
                d if d.is_ascii_digit() => width = width * 10 + d.to_digit(10).unwrap_or(0) as usize,
                'l' | 'h' => {}
                _ => break,
            }
            it.next();
        }
        let Some(conv) = it.next() else { break };
        let arg = args.get(ai);
        ai += 1;
        let s = match (conv, arg) {
            (_, None) => String::new(),
            ('s', Some(FmtArg::Str(s))) => s.clone(),
            ('c', Some(FmtArg::Int(v))) => (*v as u8 as char).to_string(),
            (_, Some(FmtArg::Int(v))) => v.to_string(),
            (_, Some(FmtArg::Str(s))) => s.clone(),
        };
        let pad = width.saturating_sub(s.chars().count());
        if left {
            out.push_str(&s);
            out.extend(std::iter::repeat_n(' ', pad));
        } else if zero && conv != 's' {
            if let Some(rest) = s.strip_prefix('-') {
                out.push('-');
                out.extend(std::iter::repeat_n('0', pad));
                out.push_str(rest);
            } else {
                out.extend(std::iter::repeat_n('0', pad));
                out.push_str(&s);
            }
        } else {
            out.extend(std::iter::repeat_n(' ', pad));
            out.push_str(&s);
        }
    }
    out
}

/// `DIKStrings[256][LANG_NUMLANGS]` (`Common/language.c:16`): english, french, german,
/// italian, american. Only the first 222 keys are initialised.
pub static DIK_STRINGS: [[&str; LANG_NUMLANGS]; 222] = [
    ["", "", "", "", ""],
    ["Escape", "Echap", "Esc", "Esc", "Escape"],
    ["1", "1", "1", "1", "1"],
    ["2", "2", "2", "2", "2"],
    ["3", "3", "3", "3", "3"],
    ["4", "4", "4", "4", "4"],
    ["5", "5", "5", "5", "5"],
    ["6", "6", "6", "6", "6"],
    ["7", "7", "7", "7", "7"],
    ["8", "8", "8", "8", "8"],
    ["9", "9", "9", "9", "9"],
    ["0", "0", "0", "0", "0"],
    ["-", ")", "\u{df}", "\u{b4}", "-"],
    ["=", "=", "\u{b4}", "\u{cc}", "="],
    ["Backspace", "Espace arri\u{e8}re", "R\u{fc}cktaste", "Indietro", "Backspace"],
    ["Tab", "Tabulation", "Tab", "Tab", "Tab"],
    ["q", "a", "q", "q", "q"],
    ["w", "z", "w", "w", "w"],
    ["e", "e", "e", "e", "e"],
    ["r", "r", "r", "r", "r"],
    ["t", "t", "t", "t", "t"],
    ["y", "y", "z", "y", "y"],
    ["u", "u", "u", "u", "u"],
    ["i", "i", "i", "i", "i"],
    ["o", "o", "o", "o", "o"],
    ["p", "p", "p", "p", "p"],
    ["[", "^", "\u{fc}", "\u{e8}", "["],
    ["]", "$", "+", "+", "]"],
    ["Return", "Entr\u{e9}e", "Eingabe", "Invio", "Enter"],
    ["Left Control", "Ctrl gauche", "Strg links", "Ctrl sinistro", "Left Control"],
    ["a", "q", "a", "a", "a"],
    ["s", "s", "s", "s", "s"],
    ["d", "d", "d", "d", "d"],
    ["f", "f", "f", "f", "f"],
    ["g", "g", "g", "g", "g"],
    ["h", "h", "h", "h", "h"],
    ["j", "j", "j", "j", "j"],
    ["k", "k", "k", "k", "k"],
    ["l", "l", "l", "l", "l"],
    [";", "m", "\u{f6}", "\u{f2}", ";"],
    ["'", "\u{f9}", "\u{e4}", "\u{e0}", "'"],
    ["`", "\u{b2}", "^", "\\", "`"],
    ["Left Shift", "Maj gauche", "Umschalt links", "Maiusc sinistro", "Left Shift"],
    ["#", "*", "#", "\u{f9}", "#"],
    ["z", "w", "y", "z", "z"],
    ["x", "x", "x", "x", "x"],
    ["c", "c", "c", "c", "c"],
    ["v", "v", "v", "v", "v"],
    ["b", "b", "b", "b", "b"],
    ["n", "n", "n", "n", "n"],
    ["m", ",", "m", "m", "m"],
    [",", ";", ";", ",", ","],
    [".", ":", ":", ".", "."],
    ["/", "!", "-", "-", "/"],
    ["Right Shift", "Maj droite", "Umschalt rechts", "Maiusc destro", "Right Shift"],
    ["*", "*", "*", "*", "*"],
    ["Left Alt", "Alt", "Alt", "Alt", "Left Alt"],
    ["Space", "Barre d'espacement", "Leertaste", "Barra spaziatrice", "Space"],
    ["Caps Lock", "Verr maj", "Feststell", "Bloc Maiusc", "Caps Lock"],
    ["F1", "F1", "F1", "F1", "F1"],
    ["F2", "F2", "F2", "F2", "F2"],
    ["F3", "F3", "F3", "F3", "F3"],
    ["F4", "F4", "F4", "F4", "F4"],
    ["F5", "F5", "F5", "F5", "F5"],
    ["F6", "F6", "F6", "F6", "F6"],
    ["F7", "F7", "F7", "F7", "F7"],
    ["F8", "F8", "F8", "F8", "F8"],
    ["F9", "F9", "F9", "F9", "F9"],
    ["F10", "F10", "F10", "F10", "F10"],
    ["NumLock", "Verr Num", "Num-Taste", "Bloc Num", "NumLock"],
    ["Scroll Lock", "Arr\u{ea}t d\u{e9}fil", "Rollen", "Bloc Scorr", "Scroll Lock"],
    ["Numpad 7", "Pavnum 7", "Zifblk 7", "Tast. num. 7", "Numpad 7"],
    ["Numpad 8", "Pavnum 8", "Zifblk 8", "Tast. num. 8", "Numpad 8"],
    ["Numpad 9", "Pavnum 9", "Zifblk 9", "Tast. num. 9", "Numpad 9"],
    ["-", "-", "-", "-", "-"],
    ["Numpad 4", "Pavnum 4", "Zifblk 4", "Tast. num. 4", "Numpad 4"],
    ["Numpad 5", "Pavnum 5", "Zifblk 5", "Tast. num. 5", "Numpad 5"],
    ["Numpad 6", "Pavnum 6", "Zifblk 6", "Tast. num. 6", "Numpad 6"],
    ["+", "+", "+", "+", "+"],
    ["Numpad 1", "Pavnum 1", "Zifblk 1", "Tast. num. 1", "Numpad 1"],
    ["Numpad 2", "Pavnum 2", "Zifblk 2", "Tast. num. 2", "Numpad 2"],
    ["Numpad 3", "Pavnum 3", "Zifblk 3", "Tast. num. 3", "Numpad 3"],
    ["Numpad 0", "Pavnum 0", "Zifblk 0", "Tast. num. 0", "Numpad 0"],
    [".", ".", ".", ".", "."],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["\\", "<", "<", "<", "\\"],
    ["F11", "F11", "F11", "F11", "F11"],
    ["F12", "F12", "F12", "F12", "F12"],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["F13", "F13", "F13", "F13", "F13"],
    ["F14", "F14", "F14", "F14", "F14"],
    ["F15", "F15", "F15", "F15", "F15"],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["Kana", "Kana", "Kana", "Kana", "Kana"],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["Convert", "Convert", "Convert", "Convert", "Convert"],
    ["", "", "", "", ""],
    ["No Convert", "No Convert", "No Convert", "No Convert", "No Convert"],
    ["", "", "", "", ""],
    ["Yen", "Yen", "Yen", "Yen", "Yen"],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["=", "=", "=", "=", "="],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["Circumflex", "Circumflex", "Circumflex", "Circumflex", "Circumflex"],
    ["@", "%", "\u{c4}", "\u{b0}", "@"],
    [":", ":", ":", ":", ":"],
    ["Underline", "Underline", "Underline", "Underline", "Underline"],
    ["Kanji", "Kanji", "Kanji", "Kanji", "Kanji"],
    ["Stop", "Stop", "Stop", "Stop", "Stop"],
    ["AX", "AX", "AX", "AX", "AX"],
    ["Unlabeled", "Unlabeled", "Unlabeled", "Unlabeled", "Unlabeled"],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["Numpad Enter", "Entr", "Enter", "Invio", "Numpad Enter"],
    ["Right Control", "Ctrl droit", "Strg rechts", "Ctrl destro", "Right Control"],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    [",", ",", ",", ",", ","],
    ["", "", "", "", ""],
    ["/", "!", "-", "-", "/"],
    ["", "", "", "", ""],
    ["SysRq", "Syst", "S-Abf", "R Sist", "SysRq"],
    ["Right Alt", "Alt Gr", "Alt Gr", "Alt Gr", "Right Alt"],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["Home", "Origine", "Pos 1", "Home", "Home"],
    ["Up", "Touche haut", "Nach-oben-Taste", "Freccia su", "Up"],
    ["Page Up", "Pg. pr\u{e9}c.", "Bild-auf", "Pgsu", "Page Up"],
    ["", "", "", "", ""],
    ["Left", "Touche gauche", "Nach-links-Taste", "Freccia sinistra", "Left"],
    ["", "", "", "", ""],
    ["Right", "Touche droite", "Nach-rechts-Taste", "Freccia destra", "Right"],
    ["", "", "", "", ""],
    ["End", "Fin", "Ende", "Fine", "End"],
    ["Down", "Touche bas", "Nach-unten-Taste", "Freccia gi\u{f9}", "Down"],
    ["Page Down", "Pg. suiv.", "Bild-ab", "Pggi\u{f9}", "Page Down"],
    ["Insert", "Inser", "Einfg", "Ins", "Insert"],
    ["Delete", "Suppr", "Entf", "Canc", "Delete"],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["", "", "", "", ""],
    ["Left Win", "Symbole Windows G", "Windowstaste links", "Tasto Win sinistro", "Left Win"],
    ["Right Win", "Symbole Windows D", "Windowstaste rechts", "Tasto Win destro", "Right Win"],
    ["Apps", "Touche d'application", "Anwendungstaste", "Tasto menu a tendina", "Apps"],
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sprintf_formats() {
        assert_eq!(c_sprintf("%d:%02d:%02d", &[FmtArg::Int(1), FmtArg::Int(5), FmtArg::Int(30)]), "1:05:30");
        assert_eq!(c_sprintf("%s wins %d%%", &[FmtArg::Str("Frogger".into()), FmtArg::Int(3)]), "Frogger wins 3%");
    }

    #[test]
    fn gamestring_range() {
        let mut l = LanguageState::default();
        let t = StringTable::parse(b"LANG_NAME\tComments\tEnglish\tFr\tDe\tIt\tUS\r\nSTR_START\tc\tStart\tF\tD\tI\tGo\r\n").unwrap();
        l.game_text_init(&t, LANG_NUM_STRINGS, LANG_US);
        assert_eq!(l.gamestring(1), "Go");
        assert_eq!(l.gamestring(0), OUT_OF_RANGE_STRING);
        assert_eq!(l.gamestring(LANG_NUM_STRINGS as i32), OUT_OF_RANGE_STRING);
        assert_eq!(l.game_text_str[0], "US");
        assert_eq!(dik_string(1, LANG_F), Some("Echap"));
        assert_eq!(dik_string(250, LANG_UK), None);
    }
}
