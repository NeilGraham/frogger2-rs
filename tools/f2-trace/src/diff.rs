//! Comparing two traces: first divergent frame and fields, with per-field
//! ignore patterns and absolute numeric tolerances.
//!
//! Field paths look like `players[0].pos[2]`, `enemies[3].flags`,
//! `cam_source[1]`; the length of a list is `enemies.len`. A pattern is
//! matched against the path with every index replaced by `[*]`
//! (`players[*].pos[*]`); it matches the path itself or any field below it
//! (`players` matches everything in `players`), and `*` in a pattern matches
//! any run of characters (`cam_*`).
//!
//! Every field belongs to a *section* ([`section_of`]): the v1 groups `core`,
//! `players`, `enemies`, `platforms`, `garibs`, `babies`, `camera`, `hud`
//! and the optional v2 sections `script`, `triggers`, `specfx`, `sprites`,
//! `tongue`, `multi` (key `mp`) (plus v2 additions to `players`, `camera`, `babies`, `hud`).
//! The report lists the first divergence per section; `Options::sections`
//! restricts the comparison. An optional v2 section that one side does not
//! have (a v1 trace, or an emitter that does not fill it yet) is not compared
//! and is counted in [`Report::missing`] instead. `v` is never compared.

use crate::TraceFrame;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    /// Pair frames by position in the file.
    #[default]
    Index,
    /// Pair frames by `act_frame_count` (frames missing on either side are reported).
    ActFrame,
}

#[derive(Clone, Debug, Default)]
pub struct Options {
    pub ignore: Vec<String>,
    /// `(pattern, max absolute difference)` for numeric fields.
    pub tolerance: Vec<(String, f64)>,
    pub align: Align,
    /// Fields listed for the first divergent frame (0 = all).
    pub max_fields: usize,
    /// Compare only these sections (empty = all), see [`section_of`].
    pub sections: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldDiff {
    pub path: String,
    pub a: Value,
    pub b: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Divergence {
    pub index_a: Option<usize>,
    pub index_b: Option<usize>,
    pub act_frame_count: u32,
    pub fields: Vec<FieldDiff>,
    /// Number of differing fields (before `max_fields` truncation).
    pub total_fields: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub frames_a: usize,
    pub frames_b: usize,
    pub compared: usize,
    pub diverged_frames: usize,
    pub first: Option<Divergence>,
    /// Per section: frames that differ in it and its first divergence.
    pub sections: BTreeMap<String, SectionReport>,
    /// Per optional section: compared frames in which only one side had it.
    pub missing: BTreeMap<String, usize>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SectionReport {
    pub diverged_frames: usize,
    /// First divergent frame of this section (`fields` truncated to `max_fields`).
    pub first: Option<Divergence>,
}

/// v1 fields by top-level key; anything else is its own section (the v2
/// section keys `script`, `triggers`, `camera`, `specfx`, `sprites`,
/// `tongue`, `babies`, `hud`, `players_ext`).
pub fn section_of(path: &str) -> &'static str {
    let top = path.split(['.', '[']).next().unwrap_or("");
    match top {
        "players" | "players_ext" => "players",
        "enemies" => "enemies",
        "platforms" => "platforms",
        "garib_count" | "garib_total" | "garibs" | "garib_ext" => "garibs",
        "babies_saved" | "num_babies" | "babies" => "babies",
        "cam_source" | "cam_target" | "camera" => "camera",
        "end_level_timer" | "hud" => "hud",
        "script" => "script",
        "triggers" => "triggers",
        "specfx" => "specfx",
        "sprites" => "sprites",
        "tongue" => "tongue",
        "mp" => "multi",
        "<frame>" => "<frame>",
        _ => "core",
    }
}

/// Every section name [`section_of`] can return (for `--sections` validation).
pub const SECTIONS: &[&str] =
    &["core", "players", "enemies", "platforms", "garibs", "babies", "camera", "hud", "script", "triggers", "specfx", "sprites", "tongue", "multi"];

/// Top-level keys of optional (v2) sections.
pub const OPTIONAL: &[&str] = &["players_ext", "garib_ext", "babies", "camera", "hud", "script", "triggers", "specfx", "sprites", "tongue", "mp"];

impl Report {
    pub fn identical(&self) -> bool {
        self.first.is_none() && self.frames_a == self.frames_b
    }
}

/// Path with indices replaced by `[*]`.
pub fn normalize(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut in_idx = false;
    for ch in path.chars() {
        match ch {
            '[' => {
                in_idx = true;
                out.push_str("[*]");
            }
            ']' => in_idx = false,
            _ if in_idx => {}
            _ => out.push(ch),
        }
    }
    out
}

fn glob(p: &[u8], s: &[u8]) -> bool {
    match p.first() {
        None => s.is_empty(),
        Some(b'*') => (0..=s.len()).any(|i| glob(&p[1..], &s[i..])),
        Some(&c) => s.first() == Some(&c) && glob(&p[1..], &s[1..]),
    }
}

/// Does `pattern` select `path` (or a parent of it)?
pub fn matches(pattern: &str, path: &str) -> bool {
    let n = normalize(path);
    let pat = normalize(pattern);
    if glob(pat.as_bytes(), n.as_bytes()) {
        return true;
    }
    // Any prefix ending at a component boundary.
    n.char_indices().filter(|&(i, c)| (c == '.' || c == '[') && i > 0).any(|(i, _)| glob(pat.as_bytes(), &n.as_bytes()[..i]))
}

fn flatten(v: &Value, path: &mut String, out: &mut Vec<(String, Value)>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                let l = path.len();
                if !path.is_empty() {
                    path.push('.');
                }
                path.push_str(k);
                flatten(x, path, out);
                path.truncate(l);
            }
        }
        Value::Array(a) => {
            let l = path.len();
            path.push_str(".len");
            out.push((path.clone(), Value::from(a.len())));
            path.truncate(l);
            for (i, x) in a.iter().enumerate() {
                let l = path.len();
                path.push_str(&format!("[{i}]"));
                flatten(x, path, out);
                path.truncate(l);
            }
        }
        _ => out.push((path.clone(), v.clone())),
    }
}

fn fields(f: &TraceFrame) -> (Vec<(String, Value)>, Vec<String>) {
    let v = serde_json::to_value(f).expect("serialisable");
    let tops = v.as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default();
    let mut out = Vec::new();
    flatten(&v, &mut String::new(), &mut out);
    (out, tops)
}

fn top(path: &str) -> &str {
    path.split(['.', '[']).next().unwrap_or("")
}

fn within(opt: &Options, path: &str, a: &Value, b: &Value) -> bool {
    if a == b {
        return true;
    }
    let (Some(x), Some(y)) = (a.as_f64(), b.as_f64()) else { return false };
    opt.tolerance.iter().any(|(p, tol)| matches(p, path) && (x - y).abs() <= *tol)
}

/// Field differences between two frames (ignores and tolerances applied).
pub fn diff_frames(a: &TraceFrame, b: &TraceFrame, opt: &Options) -> Vec<FieldDiff> {
    diff_frames_ex(a, b, opt, &mut BTreeMap::new())
}

/// [`diff_frames`], also counting optional sections present on one side only.
fn diff_frames_ex(a: &TraceFrame, b: &TraceFrame, opt: &Options, missing: &mut BTreeMap<String, usize>) -> Vec<FieldDiff> {
    let (fa, ta) = fields(a);
    let (fb, tb) = fields(b);
    let one_sided: Vec<&str> = OPTIONAL.iter().copied().filter(|k| ta.iter().any(|t| t == k) != tb.iter().any(|t| t == k)).collect();
    for k in &one_sided {
        if opt.sections.is_empty() || opt.sections.iter().any(|s| s == section_of(k)) {
            *missing.entry(k.to_string()).or_default() += 1;
        }
    }
    let mb: HashMap<&str, &Value> = fb.iter().map(|(k, v)| (k.as_str(), v)).collect();
    let ma: HashMap<&str, &Value> = fa.iter().map(|(k, v)| (k.as_str(), v)).collect();
    let mut out = Vec::new();
    let mut push = |path: &str, x: &Value, y: &Value| {
        if path == "v"
            || one_sided.contains(&top(path))
            || (!opt.sections.is_empty() && !opt.sections.iter().any(|s| s == section_of(path)))
            || opt.ignore.iter().any(|p| matches(p, path))
            || within(opt, path, x, y)
        {
            return;
        }
        out.push(FieldDiff { path: path.to_string(), a: x.clone(), b: y.clone() });
    };
    for (k, x) in &fa {
        push(k, x, mb.get(k.as_str()).copied().unwrap_or(&Value::Null));
    }
    for (k, y) in &fb {
        if !ma.contains_key(k.as_str()) {
            push(k, &Value::Null, y);
        }
    }
    out
}

/// Compare two traces.
pub fn diff(a: &[TraceFrame], b: &[TraceFrame], opt: &Options) -> Report {
    let mut rep = Report { frames_a: a.len(), frames_b: b.len(), ..Default::default() };
    let pairs: Vec<(Option<usize>, Option<usize>)> = match opt.align {
        Align::Index => (0..a.len().max(b.len())).map(|i| ((i < a.len()).then_some(i), (i < b.len()).then_some(i))).collect(),
        Align::ActFrame => {
            let ib: HashMap<u32, usize> = b.iter().enumerate().map(|(i, f)| (f.act_frame_count, i)).collect();
            let ia: HashMap<u32, usize> = a.iter().enumerate().map(|(i, f)| (f.act_frame_count, i)).collect();
            let mut v: Vec<_> = a.iter().enumerate().map(|(i, f)| (Some(i), ib.get(&f.act_frame_count).copied())).collect();
            v.extend(b.iter().enumerate().filter(|(_, f)| !ia.contains_key(&f.act_frame_count)).map(|(i, _)| (None, Some(i))));
            v
        }
    };
    for (ia, ib) in pairs {
        let d = match (ia, ib) {
            (Some(i), Some(j)) => {
                rep.compared += 1;
                diff_frames_ex(&a[i], &b[j], opt, &mut rep.missing)
            }
            _ => vec![FieldDiff {
                path: "<frame>".into(),
                a: ia.map_or(Value::Null, |_| Value::from("present")),
                b: ib.map_or(Value::Null, |_| Value::from("present")),
            }],
        };
        if d.is_empty() {
            continue;
        }
        rep.diverged_frames += 1;
        let afc = ia.map(|i| a[i].act_frame_count).or(ib.map(|j| b[j].act_frame_count)).unwrap_or(0);
        let make = |d: Vec<FieldDiff>| {
            let total = d.len();
            let mut fields = d;
            if opt.max_fields > 0 {
                fields.truncate(opt.max_fields);
            }
            Divergence { index_a: ia, index_b: ib, act_frame_count: afc, fields, total_fields: total }
        };
        let mut by_sec: BTreeMap<&str, Vec<FieldDiff>> = BTreeMap::new();
        for f in &d {
            by_sec.entry(section_of(&f.path)).or_default().push(f.clone());
        }
        for (sec, fs) in by_sec {
            let r = rep.sections.entry(sec.to_string()).or_default();
            r.diverged_frames += 1;
            if r.first.is_none() {
                r.first = Some(make(fs));
            }
        }
        if rep.first.is_none() {
            rep.first = Some(make(d));
        }
    }
    rep
}

/// Human-readable summary.
pub fn render(rep: &Report, name_a: &str, name_b: &str) -> String {
    let mut s = format!("{name_a}: {} frames, {name_b}: {} frames, {} compared\n", rep.frames_a, rep.frames_b, rep.compared);
    match &rep.first {
        None if rep.frames_a == rep.frames_b => s.push_str("identical\n"),
        None => s.push_str("no differing fields, but the frame counts differ\n"),
        Some(d) => {
            let idx = |i: Option<usize>| i.map_or("-".to_string(), |i| i.to_string());
            s.push_str(&format!(
                "first divergence: line {} / {} (act_frame_count {}), {} field(s); {} frame(s) differ in total\n",
                idx(d.index_a),
                idx(d.index_b),
                d.act_frame_count,
                d.total_fields,
                rep.diverged_frames
            ));
            for f in &d.fields {
                s.push_str(&format!("  {}: {} != {}\n", f.path, f.a, f.b));
            }
            if d.fields.len() < d.total_fields {
                s.push_str(&format!("  ... {} more\n", d.total_fields - d.fields.len()));
            }
            s.push_str("per section (first divergence, frames differing):\n");
            for (name, r) in &rep.sections {
                let Some(f) = &r.first else { continue };
                let first = f.fields.first().map_or(String::new(), |x| format!(": {} {} != {}", x.path, x.a, x.b));
                s.push_str(&format!(
                    "  {name:<9} act_frame_count {} (line {}), {} field(s), {} frame(s){first}\n",
                    f.act_frame_count,
                    idx(f.index_a),
                    f.total_fields,
                    r.diverged_frames
                ));
            }
        }
    }
    for (k, n) in &rep.missing {
        s.push_str(&format!("note: section {k} present on one side only in {n} compared frame(s); not compared\n"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample;

    #[test]
    fn patterns() {
        assert_eq!(normalize("players[0].pos[2]"), "players[*].pos[*]");
        assert!(matches("players", "players[0].pos[2]"));
        assert!(matches("players[*].pos", "players[1].pos[0]"));
        assert!(matches("cam_*", "cam_target[2]"));
        assert!(matches("rseed", "rseed"));
        assert!(!matches("rseed", "rseed2"));
        assert!(!matches("players[*].pos", "players[0].pos_x"));
        assert!(!matches("pos", "players[0].pos[0]"));
        assert!(matches("*.pos", "players[0].pos[0]"));
    }

    #[test]
    fn identical_and_first_divergence() {
        let a: Vec<_> = (0..10).map(sample).collect();
        let mut b = a.clone();
        assert!(diff(&a, &b, &Options::default()).identical());
        b[4].players[0].pos = Some([-499, 0, 10250]);
        b[6].rseed ^= 1;
        b[6].enemies.pop();
        let r = diff(&a, &b, &Options::default());
        assert_eq!(r.diverged_frames, 2);
        let d = r.first.clone().unwrap();
        assert_eq!((d.index_a, d.act_frame_count), (Some(4), 5));
        assert_eq!(d.fields, vec![FieldDiff { path: "players[0].pos[0]".into(), a: (-500).into(), b: (-499).into() }]);
        let text = render(&r, "a", "b");
        assert!(text.contains("first divergence: line 4 / 4 (act_frame_count 5)"), "{text}");

        // Tolerance absorbs the position, ignore absorbs rseed; the enemy list still differs.
        let opt = Options { tolerance: vec![("players[*].pos".into(), 1.0)], ignore: vec!["rseed".into()], ..Default::default() };
        let r = diff(&a, &b, &opt);
        let d = r.first.unwrap();
        assert_eq!(d.index_a, Some(6));
        assert!(d.fields.iter().any(|f| f.path == "enemies.len"));
        assert!(d.fields.iter().any(|f| f.path.starts_with("enemies[1].") && f.b == Value::Null));
    }

    #[test]
    fn sections_v2_and_filter() {
        use crate::sample_v2;
        let a: Vec<_> = (0..6).map(sample_v2).collect();
        // v1 vs v2: sections only one side has are not compared, `v` is never compared.
        let v1: Vec<_> = (0..6).map(sample).collect();
        let r = diff(&a, &v1, &Options::default());
        assert!(r.identical(), "{}", render(&r, "a", "b"));
        assert_eq!(r.missing.get("script"), Some(&6));
        assert!(render(&r, "a", "b").contains("note: section script present on one side only in 6"));

        let mut b = a.clone();
        b[2].script.as_mut().unwrap().flags[3] = 8;
        b[4].triggers.as_mut().unwrap().list[0].count = 5;
        b[4].rseed ^= 1;
        let r = diff(&a, &b, &Options::default());
        assert_eq!(r.diverged_frames, 2);
        assert_eq!(r.sections["script"].first.as_ref().unwrap().act_frame_count, 3);
        assert_eq!(r.sections["triggers"].first.as_ref().unwrap().fields[0].path, "triggers.list[0].count");
        assert_eq!(r.sections["core"].diverged_frames, 1);
        let text = render(&r, "a", "b");
        assert!(text.contains("  script    act_frame_count 3 (line 2), 1 field(s), 1 frame(s): script.flags[3] 0 != 8"), "{text}");

        let opt = Options { sections: vec!["triggers".into()], ..Default::default() };
        let r = diff(&a, &b, &opt);
        assert_eq!(r.diverged_frames, 1);
        assert_eq!(r.first.unwrap().fields[0].path, "triggers.list[0].count");
        assert_eq!(section_of("garib_ext.drop_speed[0]"), "garibs");
        assert_eq!(section_of("players_ext[0].frog_facing"), "players");
        assert_eq!(section_of("rseed"), "core");
    }

    #[test]
    fn alignment_and_lengths() {
        let a: Vec<_> = (0..5).map(sample).collect();
        let b: Vec<_> = (1..5).map(sample).collect();
        let r = diff(&a, &b, &Options::default());
        assert_eq!(r.first.unwrap().index_a, Some(0));
        let opt = Options { align: Align::ActFrame, ignore: vec!["seq".into()], ..Default::default() };
        let r = diff(&a, &b, &opt);
        let d = r.first.unwrap();
        assert_eq!((d.index_a, d.index_b), (Some(0), None));
        assert_eq!(r.diverged_frames, 1);
        assert_eq!(r.compared, 4);
    }
}
