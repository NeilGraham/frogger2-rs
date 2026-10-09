use f2_platform::config::Config;
use f2_platform::dik;
use f2_platform::input::*;

#[test]
fn pad_bits_match_islpad() {
    assert_eq!(PAD_SELECT, 0x0001);
    assert_eq!(PAD_START, 0x0008);
    assert_eq!(PAD_UP, 0x0010);
    assert_eq!(PAD_LEFT, 0x0080);
    assert_eq!(PAD_R1, 0x0800);
    assert_eq!(PAD_CROSS, 0x4000);
    assert_eq!(PAD_SQUARE, 0x8000);
    for b in 0..16 {
        assert_eq!(parse_pad_bit(pad_bit_name(1 << b).unwrap()), Some(1 << b));
    }
}

#[test]
fn default_keymap_shape() {
    let k = Keymap::default();
    for (i, e) in k.0.iter().enumerate() {
        assert_eq!(e.player as usize, i / KEYS_PER_PLAYER);
        assert_eq!(e.button.count_ones(), 1);
    }
    assert_eq!(k.0[0], KeyEntry { player: 0, button: PAD_UP, key: 0xC8 });
    assert_eq!(k.0[4], KeyEntry { player: 0, button: PAD_CROSS, key: 0x1C });
    assert_eq!(k.0[14 + 7], KeyEntry { player: 1, button: PAD_CIRCLE, key: dik::Q });
    assert_eq!(k.0[55], KeyEntry { player: 3, button: PAD_R1, key: 0 });
}

#[test]
fn default_keymap_round_trips() {
    let k = Keymap::default();
    // Registry binary (KeyboardDump).
    let bytes = k.to_registry_bytes();
    assert_eq!(&bytes[..6], &[0, 4, 0xC8, 0, 6, 0xD0]);
    assert_eq!(Keymap::from_registry_bytes(&bytes), Some(k.clone()));
    assert_eq!(Keymap::from_registry_bytes(&bytes[..100]), None);
    // TOML bindings.
    let b = k.to_bindings();
    assert_eq!(b[4].button, "CROSS");
    assert_eq!(b[4].key, "RETURN");
    assert_eq!(b[20].key, "NONE");
    assert_eq!(Keymap::from_bindings(&b).unwrap(), k);
    // Whole config through TOML text.
    let mut c = Config::default();
    c.input.controllers[1] = Controller::Joypad(0);
    c.audio.music = 40;
    let s = c.to_toml();
    let c2 = Config::from_toml(&s).unwrap();
    assert_eq!(c2, c);
    assert_eq!(c2.keymap(), k);
    // Rebinding survives.
    let mut k2 = k.clone();
    assert!(k2.bind(1, PAD_START, dik::SPACE));
    c.set_keymap(&k2);
    assert_eq!(Config::from_toml(&c.to_toml()).unwrap().keymap(), k2);
    // Partial config files fill in defaults.
    let p = Config::from_toml("language = \"uk\"\n[video]\nfullscreen = true\n").unwrap();
    assert_eq!(p.language, "uk");
    assert!(p.video.fullscreen);
    assert_eq!(p.video.width, 640);
    assert_eq!(p.keymap(), k);
}

#[test]
fn dik_names() {
    for &(c, n) in dik::NAMES {
        assert_eq!(dik::parse(n), Some(c));
        assert_eq!(dik::name(c), n);
    }
    assert_eq!(dik::parse("DIK_NUMPADENTER"), Some(0x9C));
    assert_eq!(dik::parse("8"), Some(dik::KEY_8));
    assert_eq!(dik::parse("0x7F"), Some(0x7F));
    assert_eq!(dik::name(0x7F), "0x7F");
}

#[test]
fn keyboard_maps_to_players() {
    let m = InputMapper::default();
    let mut pad = PadData::default();
    let mut kb = KeyboardState::default();
    kb.key_down(dik::UP);
    kb.key_down(dik::Q); // player 2: UP and CIRCLE
    kb.key_down(dik::NUMPADENTER); // player 4 CROSS
    m.process(&mut pad, &kb, &[]);
    assert_eq!(pad.digital[0], PAD_UP);
    assert_eq!(pad.digital[1], PAD_UP | PAD_CIRCLE);
    assert_eq!(pad.digital[3], PAD_CROSS);
    assert_eq!(pad.debounce[1], PAD_UP | PAD_CIRCLE);
    kb.end_frame();
    // Held: no debounce on the next frame.
    m.process(&mut pad, &kb, &[]);
    assert_eq!(pad.digital[0], PAD_UP);
    assert_eq!(pad.debounce[0], 0);
    // ESCAPE is both START and TRIANGLE for player 1, TRIANGLE for the others.
    kb.key_down(dik::ESCAPE);
    m.process(&mut pad, &kb, &[]);
    assert_eq!(pad.debounce[0], PAD_START | PAD_TRIANGLE);
    assert_eq!(pad.digital[2], PAD_TRIANGLE);
}

#[test]
fn tap_within_a_frame_still_debounces() {
    // Buffered key-down events reach debounce even if the key is up again at poll time.
    let m = InputMapper::default();
    let mut pad = PadData::default();
    let mut kb = KeyboardState::default();
    kb.key_down(dik::RETURN);
    kb.key_up(dik::RETURN);
    m.process(&mut pad, &kb, &[]);
    assert_eq!(pad.digital[0], 0);
    assert_eq!(pad.debounce[0], PAD_CROSS);
}

#[test]
fn menu_keys_and_focus() {
    let mut m = InputMapper::new(Keymap::default());
    let mut k = m.keymap.clone();
    k.bind(0, PAD_UP, dik::W);
    m.keymap = k;
    let mut pad = PadData::default();
    let mut kb = KeyboardState::default();
    kb.down[dik::UP as usize] = true;
    m.process(&mut pad, &kb, &[]);
    assert_eq!(pad.digital[0], 0, "UP was rebound away");
    m.check_menu_keys = true;
    m.process(&mut pad, &kb, &[]);
    assert_eq!(pad.digital[0], PAD_UP, "menu keys always work");
    m.window_active = false;
    m.process(&mut pad, &kb, &[]);
    assert_eq!(pad.digital[0], 0);
}

#[test]
fn joypads() {
    let mut m = InputMapper::default();
    m.reset_controllers(1);
    assert_eq!(m.controllers, [Controller::Joypad(0), Controller::Keyboard, Controller::Keyboard, Controller::Keyboard]);
    let j = JoypadState { buttons: [true, false, false, false, true, false], x: -600, y: 400, extra: PAD_L1 };
    assert_eq!(joypad_bits(&j), PAD_CROSS | PAD_START | PAD_LEFT | PAD_L1);
    let mut pad = PadData::default();
    m.process(&mut pad, &KeyboardState::default(), &[Some(j)]);
    assert_eq!(pad.digital[0], PAD_CROSS | PAD_START | PAD_LEFT | PAD_L1);
    assert_eq!(pad.debounce[0], pad.digital[0]);
    assert_eq!(pad.digital[1], 0);
    let j2 = JoypadState { y: -501, ..Default::default() };
    m.process(&mut pad, &KeyboardState::default(), &[Some(j2)]);
    assert_eq!(pad.digital[0], PAD_UP);
    assert_eq!(pad.debounce[0], PAD_UP);
}

#[test]
fn data_root_matches_importer() {
    assert_eq!(f2_platform::paths::default_data_root(), f2_import::default_data_root());
    assert!(f2_platform::paths::config_file().ends_with("config.toml"));
}
