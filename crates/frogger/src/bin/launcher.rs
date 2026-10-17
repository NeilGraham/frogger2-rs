//! `launcher`: imports the game, starts it, installs other versions, manages mods and makes the
//! game's shortcut (docs/USER-GUIDE.md "Launcher"). The app itself is [`frogger::launcher`].

fn main() {
    if !frogger::launcher::run() {
        std::process::exit(1);
    }
}
