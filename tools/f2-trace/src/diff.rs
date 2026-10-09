//! Comparing two traces: first divergent frame and fields, with per-field
//! ignore patterns and absolute numeric tolerances.
//!
//! Field paths look like `players[0].pos[2]`, `enemies[3].flags`,
//! `cam_source[1]`; the length of a list is `enemies.len`. A pattern is
//! matched against the path with every index replaced by `[*]`
//! (`players[*].pos[*]`); it matches the path itself or any field below it
//! (`players` matches everything in `players`), and `*` in a pattern matches
//! any run of characters (`cam_*`).

use crate::TraceFrame;
use serde_json::Value;
use std::collections::HashMap;

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
}

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

fn fields(f: &TraceFrame) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    flatten(&serde_json::to_value(f).expect("serialisable"), &mut String::new(), &mut out);
    out
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
    let fa = fields(a);
    let fb = fields(b);
    let mb: HashMap<&str, &Value> = fb.iter().map(|(k, v)| (k.as_str(), v)).collect();
    let ma: HashMap<&str, &Value> = fa.iter().map(|(k, v)| (k.as_str(), v)).collect();
    let mut out = Vec::new();
    let mut push = |path: &str, x: &Value, y: &Value| {
        if opt.ignore.iter().any(|p| matches(p, path)) || within(opt, path, x, y) {
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
                diff_frames(&a[i], &b[j], opt)
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
        if rep.first.is_none() {
            let total = d.len();
            let mut fields = d;
            if opt.max_fields > 0 {
                fields.truncate(opt.max_fields);
            }
            let afc = ia.map(|i| a[i].act_frame_count).or(ib.map(|j| b[j].act_frame_count)).unwrap_or(0);
            rep.first = Some(Divergence { index_a: ia, index_b: ib, act_frame_count: afc, fields, total_fields: total });
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
        }
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
