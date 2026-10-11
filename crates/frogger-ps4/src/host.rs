//! Platform-independent parts of the console host. The shared ones (pad conversion, the
//! framebuffer scalers, dynamic resolution, data and save dirs) live in
//! `crates/console-host` (re-exported here); this adds the PS4 host's start-up script.

pub use console_host::host::*;
use frogger::app::Step;

/// Parses `autostart.txt` (testing without pad input): `level W L`, `demo W L`,
/// `frontend` or `boot`. Anything else: `None` (normal start-up).
pub fn autostart(text: &str) -> Option<Step> {
    let mut it = text.split_whitespace();
    let mut num = || it.next().and_then(|n| n.parse::<u32>().ok());
    match text.split_whitespace().next()? {
        "level" => {
            num();
            Some(Step::Level(num()?, num()?))
        }
        "demo" => {
            num();
            Some(Step::Demo(num()?, num()?))
        }
        "frontend" => Some(Step::Frontend),
        "boot" => Some(Step::Boot),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autostart_parse() {
        assert!(matches!(autostart("level 2 1\n"), Some(Step::Level(2, 1))));
        assert!(matches!(autostart("frontend"), Some(Step::Frontend)));
        assert!(autostart("level x").is_none());
        assert!(autostart("").is_none());
    }
}
