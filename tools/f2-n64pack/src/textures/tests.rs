use std::path::Path;

use f2_formats::texture::{TexAnim, TexAnimFrame, TexBank, Texture, TEX_TRANSVAL};

use super::*;

fn tex(name: &str, flags: u32, w: u16, h: u16, f: impl Fn(usize, usize) -> u16) -> Texture {
    let pixels = (0..h as usize).flat_map(|y| (0..w as usize).map(move |x| (x, y))).map(|(x, y)| f(x, y)).collect();
    Texture { flags, crc: f2_formats::crc(name), width: w, height: h, name: name.into(), pixels }
}

/// A pseudo-random 15-bit colour (many distinct colours).
fn noise(x: usize, y: usize) -> u16 {
    let mut v = (x as u32).wrapping_mul(0x9E37_79B9) ^ (y as u32).wrapping_mul(0x85EB_CA6B);
    v ^= v >> 13;
    v = v.wrapping_mul(0xC2B2_AE35);
    ((v >> 7) & 0x7fff) as u16
}

fn smooth(x: usize, y: usize) -> u16 {
    let r = (x * 31 / 63) as u16;
    let g = (y * 31 / 63) as u16;
    let b = ((x + y) * 31 / 126) as u16;
    (r << 10) | (g << 5) | b
}

fn sources() -> Sources {
    let few = |x: usize, y: usize| [0x001f, 0x7c00, TEX_TRANSVAL, 0x03e0][(x / 4 + y / 4) % 4];
    let shared = tex("shared.bmp", 0, 32, 32, smooth);
    let mut shared2 = shared.clone();
    shared2.name = "shared2.bmp".into();
    shared2.crc = f2_formats::crc("shared2.bmp");
    let a = TexBank {
        textures: vec![
            tex("few.bmp", 0, 32, 32, few),
            tex("ai_mask.bmp", 1, 32, 32, |x, _| (x as u16) & 0x1f),
            tex("prc_fire1.bmp", 2, 32, 32, noise),
            tex("big3d.bmp", 0, 64, 64, noise),
            tex("smooth3d.bmp", 0, 64, 64, smooth),
            tex("logo.bmp", 0, 256, 128, noise),
            shared.clone(),
            shared2,
        ],
        anims: vec![TexAnim {
            base_crc: f2_formats::crc("few.bmp"),
            frames: vec![TexAnimFrame { crc: f2_formats::crc("few.bmp"), time: 3 }, TexAnimFrame { crc: f2_formats::crc("shared.bmp"), time: -1 }],
        }],
    };
    let b = TexBank { textures: vec![shared.clone(), tex("own.bmp", 0, 16, 16, noise)], anims: vec![] };
    let font = TexBank { textures: vec![tex("smallfont.bmp", 0, 40, 8, |x, y| if x % 5 == 4 || y == 0 { 0 } else { 0x7fff })], anims: vec![] };
    let mut s = Sources::default();
    s.banks.insert("alpha".into(), (a, 1000));
    s.banks.insert("beta".into(), (b, 100));
    s.banks.insert("font".into(), (font, 10));
    s.backdrops.insert("title".into(), (tex("p", 0, 64, 48, smooth), 500));
    s.used_3d.extend([f2_formats::crc("big3d.bmp"), f2_formats::crc("smooth3d.bmp"), f2_formats::crc("few.bmp")]);
    // loose copy of the font: identical → alias
    let f = s.banks["font"].0.textures[0].clone();
    s.loose.push(("font/smallfont.bmp".into(), f, 99));
    s
}

fn small_policy() -> Policy {
    Policy { backdrop_size: (32, 24), ..Policy::default() }
}

fn find(out: &Output, path: &str) -> NtxFile {
    NtxFile::parse(&out.assets.iter().find(|a| a.path == path).unwrap_or_else(|| panic!("missing {path}")).data).unwrap()
}

#[test]
fn synthetic_conversion_verifies() {
    let src = sources();
    let out = convert_sources(&src, &small_policy(), 4).unwrap();
    let rep = verify(&src, &out.assets).unwrap();
    assert_eq!(rep.entries, 11);
    assert!(rep.lossy >= 1, "big3d must be lossy");
    assert!(rep.single_tile_checked >= 3);
    let alpha = find(&out, "textures/alpha.ntx");
    let img = |crc_name: &str| {
        let e = alpha.find(f2_formats::crc(crc_name)).unwrap();
        assert_eq!(e.dep, 0);
        alpha.images[e.image as usize].clone()
    };
    assert_eq!(img("few.bmp").fmt, Fmt::Ci4);
    assert_eq!(img("ai_mask.bmp").fmt, Fmt::I4);
    assert!(img("ai_mask.bmp").flags & flag::WHITE != 0);
    let p = img("prc_fire1.bmp");
    assert_eq!((p.fmt, p.flags & flag::SOURCE_555 != 0), (Fmt::Rgba16, true));
    // 64x64 with ~4096 colours, UV-mapped: no lossless format fits → lossy single tile.
    let b = img("big3d.bmp");
    assert!(b.flags & flag::LOSSY != 0 && b.flags & flag::SINGLE_TILE != 0 && b.fmt.fits(b.width as usize, b.height as usize));
    // 2D 256x128 noise: lossless RGBA16, tiled.
    let l = img("logo.bmp");
    assert_eq!(l.fmt, Fmt::Rgba16);
    assert!(l.flags & flag::LOSSY == 0 && l.flags & flag::SINGLE_TILE == 0);
    assert!(l.fmt.fits(l.tile_w as usize, l.tile_h as usize));
    // animation table kept, base flagged
    assert_eq!(alpha.anims, src.banks["alpha"].0.anims);
    assert_eq!(alpha.find(f2_formats::crc("few.bmp")).unwrap().anim, 0);
    // font glyph table present, loose BMP aliased
    let font = find(&out, "textures/font.ntx");
    let g = &font.glyphs[font.images[0].aux as usize];
    assert_eq!(g[b'A' as usize], (0, 4));
    assert_eq!(g[b'B' as usize], (5, 4));
    assert!(!out.assets.iter().any(|a| a.path.starts_with("textures/font/")));
    assert_eq!(out.stats.loose_aliased, 1);
    // backdrop downscaled to the policy size
    let bd = find(&out, "textures/backdrop/title.ntx");
    assert_eq!((bd.images[0].width, bd.images[0].height, bd.images[0].orig_width), (32, 24, 64));
    // every produced file is 8-byte sized and all images fit their tiles
    for a in &out.assets {
        assert_eq!(a.data.len() % 8, 0, "{}", a.path);
        assert_eq!(a.class, Class::Textures);
    }
}

#[test]
fn dedupe_shares_images_across_and_within_banks() {
    let src = sources();
    let out = convert_sources(&src, &small_policy(), 2).unwrap();
    let alpha = find(&out, "textures/alpha.ntx");
    let beta = find(&out, "textures/beta.ntx");
    let pool_path = format!("shared/s{:08x}.ntx", crate::tools::fnv64(b"alpha+beta") as u32);
    assert_eq!(alpha.deps, vec![pool_path.clone()]);
    assert_eq!(beta.deps, vec![pool_path.clone()]);
    let pool = find(&out, &format!("textures/{pool_path}"));
    assert_eq!(pool.file_kind, ntx::file_kind::POOL);
    assert_eq!(pool.images.len(), 1, "one copy of the shared image");
    let ea = alpha.find(f2_formats::crc("shared.bmp")).unwrap();
    let ea2 = alpha.find(f2_formats::crc("shared2.bmp")).unwrap();
    let eb = beta.find(f2_formats::crc("shared.bmp")).unwrap();
    assert_eq!((ea.dep, ea.image), (1, 0));
    assert_eq!((ea2.dep, ea2.image), (1, 0), "same pixels under another CRC reuse the image");
    assert_eq!((eb.dep, eb.image), (1, 0));
    // shared image used 3 times, stored once
    assert_eq!(out.stats.dedupe_refs, 2);
    // the pooled image decodes to the target of every user
    let t = &src.banks["beta"].0.textures[0];
    assert_eq!(pool.decode_rgba8(0), source::target(t, kind::NORMAL));
}

#[test]
fn lossless_transparency_bleed_round_trips() {
    // magenta holes inside colour: the decoded RGB of transparent texels is the PC bleed
    let t = tex("holes.bmp", 0, 32, 32, |x, y| if (x + y) % 3 == 0 { TEX_TRANSVAL } else { noise(x, y) });
    let c = convert_image(&t, kind::NORMAL, true, &Policy::default());
    assert!(!c.lossy);
    let mut f = NtxFile::new(ntx::file_kind::SINGLE);
    f.add_image(&c.enc, &c.meta);
    let g = NtxFile::parse(&f.to_bytes()).unwrap();
    assert_eq!(g.decode_rgba8(0), Rgba::from_bytes(32, 32, &t.to_rgba8()));
    assert!(g.images[0].flags & flag::ALPHA_1BIT != 0);
}

#[test]
fn downscale_keeps_uv_scale_and_quality() {
    // smooth 128x128 gradient, UV-mapped: must downscale, PSNR should stay high
    let t = tex("grad.bmp", 0, 128, 128, |x, y| smooth(x / 2, y / 2));
    let c = convert_image(&t, kind::NORMAL, true, &Policy::default());
    assert!(c.enc.fmt.fits(c.enc.w, c.enc.h));
    assert_eq!((c.meta.orig_width, c.meta.orig_height), (128, 128));
    // hard 5-bit steps every 4 texels: the best fitting candidate (RGBA16 32x64) scores ~28.6 dB
    assert!(c.lossy && c.psnr > 27.0, "{}", c.psnr);
    assert_eq!((c.enc.fmt, c.enc.w, c.enc.h), (Fmt::Rgba16, 32, 64));
    let near = Policy { near_lossless_db: Some(30.0), ..Policy::default() };
    let small = tex("s.bmp", 0, 32, 32, smooth);
    let lossless = convert_image(&small, kind::NORMAL, true, &Policy::default());
    let tuned = convert_image(&small, kind::NORMAL, true, &near);
    assert!(tuned.enc.bytes() <= lossless.enc.bytes());
}

#[test]
fn per_tile_palettes_round_trip() {
    let (w, h) = (80, 48);
    let src: Vec<[f32; 4]> = (0..w * h).map(|i| [(i % w * 3) as f32, (i / w * 5) as f32, ((i % w + i / w) * 2) as f32, 255.0]).collect();
    let tile = rdp::palette_tile_shape(w, h);
    assert!(Fmt::Ci8.fits(tile.0, tile.1) && w % tile.0 == 0 && h % tile.1 == 0, "{tile:?}");
    let e = rdp::encode_ci8_tiled(w, h, &src, tile, Dither::FloydSteinberg);
    assert_eq!(e.tlut.len(), 256 * (w / tile.0) * (h / tile.1));
    let mut f = NtxFile::new(ntx::file_kind::BACKDROP);
    f.add_image(&e, &ImageMeta { kind: kind::BACKDROP, flags: flag::LOSSY, orig_width: w as u16, orig_height: h as u16, tile, glyphs: None });
    let g = NtxFile::parse(&f.to_bytes()).unwrap();
    assert!(g.images[0].flags & flag::TLUT_PER_TILE != 0);
    assert_eq!(g.decode_rgba8(0), e.decode(false));
    // close to the source: every tile has its own 256 colours
    let src8 = image::round_img(w, h, &src);
    assert!(image::psnr(&src8, &e.decode(false)) > 35.0);
}

/// Real data (skipped without an imported rendition): full conversion + verification.
#[test]
fn real_data_converts_and_verifies() {
    let Some(root) = crate::ctx::find_rendition(Path::new(env!("CARGO_MANIFEST_DIR"))) else {
        eprintln!("skipped: no imported rendition");
        return;
    };
    let src = Sources::load(&root.join("game")).unwrap();
    let jobs = std::thread::available_parallelism().map_or(4, |n| n.get());
    let out = convert_sources(&src, &Policy::default(), jobs).unwrap();
    let rep = verify(&src, &out.assets).unwrap();
    for n in &out.notes {
        eprintln!("{n}");
    }
    eprintln!("{rep:?}");
    for a in out.assets.iter().filter(|a| !a.path.contains("backdrop/")) {
        for n in a.notes.iter().filter(|n| n.contains("lossy") || n.contains("tiles")) {
            eprintln!("{}: {n}", a.path);
        }
    }
    assert!(rep.exact > 0 && rep.entries == out.stats.textures);
    if let Some(dir) = std::env::var_os("F2_N64_TEX_OUT") {
        for a in &out.assets {
            let p = Path::new(&dir).join(&a.path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, &a.data).unwrap();
        }
    }
    if let Some(dir) = std::env::var_os("F2_N64_TEX_PREVIEW") {
        let n = write_previews(&out.assets, Path::new(&dir)).unwrap();
        eprintln!("wrote {n} previews to {}", Path::new(&dir).display());
    }
}
