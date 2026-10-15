//! Desktop app icon and title: ours until game data exists, then a rounded square crop of the
//! player's own game artwork (`frogger::icon`) and the title from their disc
//! (`frogger::export::title`). The window/taskbar icon is set at run time; the
//! artwork icon is also written to `<config>/icons/` (`game-art.png`, `game-art.ico`) and, on
//! Windows, the game's shortcuts (Desktop, Start menu, install folder) are pointed at it.

use f2_platform::window::{winit, Window};
use frogger::{data, icon};

/// Sets `img` as the window's icon (title bar, taskbar, Alt+Tab; ignored on Wayland).
pub fn set(window: &Window, img: &icon::Rgba) {
    match winit::window::Icon::from_rgba(img.pixels.clone(), img.width, img.height) {
        Ok(i) => window.set_window_icon(Some(i)),
        Err(e) => eprintln!("frogger: window icon: {e}"),
    }
}

/// Our own icon on `window`.
pub fn set_default(window: &Window) {
    set(window, &icon::square(&icon::app_icon(), 64));
}

/// Switches to the player's game artwork when their data has it.
pub fn use_game_art(window: &Window, data: &data::Data) {
    let Some(art) = icon::game_art(data) else {
        set_default(window);
        return;
    };
    let icon_at = |size: u32| icon::rounded(&icon::square(&art, size), 0.18);
    set(window, &icon_at(64));
    // The window title from the player's disc ("Frogger 2: Swampy's Revenge").
    if let Ok(title) = frogger::export::title(data) {
        window.set_title(&title);
    }
    let dir = f2_platform::paths::config_dir().join("icons");
    let ico = dir.join("game-art.ico");
    let bytes = icon::encode_ico(&[16, 24, 32, 48, 64, 128, 256].map(icon_at));
    // Written once per change (the import is the only thing that changes it).
    if std::fs::read(&ico).is_ok_and(|b| b == bytes) {
        return;
    }
    let res = std::fs::create_dir_all(&dir)
        .and_then(|_| std::fs::write(&ico, &bytes))
        .and_then(|_| std::fs::write(dir.join("game-art.png"), icon::encode_png(&icon_at(256))));
    match res {
        Ok(()) => {
            eprintln!("frogger: app icon from your game artwork: {}", ico.display());
            update_shortcuts(&ico);
        }
        Err(e) => eprintln!("frogger: writing {}: {e}", ico.display()),
    }
}

/// Points this exe's shortcuts at `ico` (Windows; a hidden PowerShell on a worker thread).
#[cfg(windows)]
fn update_shortcuts(ico: &std::path::Path) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let (Ok(exe), Some(ico)) = (std::env::current_exe(), ico.to_str().map(str::to_string)) else { return };
    let (Some(exe), Some(dir)) = (exe.to_str().map(str::to_string), exe.parent().and_then(|d| d.to_str()).map(str::to_string)) else { return };
    let q = |s: &str| s.replace('\'', "''");
    let script = format!(
        "$s = New-Object -ComObject WScript.Shell; \
         $dirs = @([Environment]::GetFolderPath('Desktop'), [Environment]::GetFolderPath('Programs'), '{dir}'); \
         foreach ($d in $dirs) {{ Get-ChildItem -LiteralPath $d -Filter *.lnk -ErrorAction SilentlyContinue | ForEach-Object {{ \
           $l = $s.CreateShortcut($_.FullName); \
           if ($l.TargetPath -ieq '{exe}') {{ $l.IconLocation = '{ico},0'; $l.Save() }} }} }}",
        dir = q(&dir),
        exe = q(&exe),
        ico = q(&ico),
    );
    std::thread::spawn(move || {
        let r = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &script])
            .creation_flags(CREATE_NO_WINDOW)
            .status();
        if let Err(e) = r {
            eprintln!("frogger: updating shortcut icons: {e}");
        }
    });
}

#[cfg(not(windows))]
fn update_shortcuts(_ico: &std::path::Path) {}
