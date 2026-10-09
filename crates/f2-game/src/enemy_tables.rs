//! Enemy data tables whose initialisers were `#include`d from files missing from the source
//! snapshot (`x:/teamspirit/pcversion/nmehack.txt` and `.../reactive.txt`, enemies.c:93-117),
//! recovered from the USA retail `frogger2.exe` (image base 0x400000, `.data`).
//!
//! # `enemyAnimHack` (`NMEHACK[]`, enemies.c:93)
//! - Retail VA `0x00471318`, 8 bytes/entry (`char *modelname; short flags;` + 2 pad), 17 entries
//!   plus a terminator `{NULL, 0}` at `0x004713a0` (144 bytes total).
//! - Located via `AddEnemyModelUpdates` (enemies.c:2080), inlined into `CreateAndAddEnemy`
//!   (`0x0040a5b0`): a `while (hack->modelname)` loop over `0x00471318` with stride 8 that stores
//!   `hackFuncs[hack->flags]` (`hackFuncs` at `0x004712f8`, 7 pointers) into `nme->Update[1]`.
//!   The loop stops at the NULL `modelname` terminator (no count).
//! - `flags` is an index into `hackFuncs` (enemies.c:86); the update function is noted per row.
//!   Matching is `*c == tolower(*d)` on the hack name, up to the `.` in the actor name.
//!
//! # `reactiveAnims` (`REACTIVEANIM[]`, enemies.c:111)
//! - Retail VA `0x004713a8` (immediately after `enemyAnimHack`), 24 bytes/entry
//!   (`char name[16]; short animChar; short animFrog; long type;`), 69 entries plus an all-zero
//!   terminator (empty name) at `0x00471a20` (1680 bytes total).
//! - Located via `GetReactiveAnimNumber` (enemies.c:153), inlined into `CreateAndAddEnemy`
//!   (`0x0040a5b0`): loop over `0x004713a8` with stride 0x18 calling gstrlwr/gstrcmp until
//!   `name[0] == 0`; and `NMEDamageFrog` (`0x004066a0`), which reads `type` (+0x14),
//!   `animFrog` (+0x12, `deathAnims[]` index) and `animChar` (+0x10).
//! - Names are stored as in the exe; note the original gstrlwr()s them in place at lookup time
//!   (e.g. `NME.obe`, `Labspy.obe`, `smineB.obe` are lowercased at runtime). `stomp.obe` appears
//!   twice (index 35 and 37); the lookup returns the first match, so 37 is unreachable by name.
//! - `type` holds ANI_* flags (ANI_FACE=1, ANI_CENTRE=2, ANI_FIXED=4, ANI_ANTIFACE=8,
//!   ANI_FACECAMERA=16, ANI_REACTIVE=1<<31). No retail entry has ANI_REACTIVE, so `animFrog` is
//!   always a DEATHBY_* index (frogmove.h:69) and `animChar` is -1 except `bee.obe`.

/// `enemyAnimHack[]` (model name prefix, `hackFuncs` index), terminator `{NULL, 0}` omitted.
pub const ORIG_ENEMY_ANIM_HACK: &[(&str, i16)] = &[
    ("moa", 0),      // UpdateJigglingEnemy
    ("roll", 0),     // UpdateJigglingEnemy
    ("beetle", 0),   // UpdateJigglingEnemy
    ("warthog", 2),  // UpdateGallopingEnemy
    ("stnwheel", 3), // UpdateRollZEnemy
    ("boulder", 5),  // UpdateRollEnemy
    ("labspy", 1),   // UpdateBobbingEnemy
    ("pkin", 4),     // UpdateBouncingEnemy
    ("cnut", 4),     // UpdateBouncingEnemy
    ("ghostie", 2),  // UpdateGallopingEnemy
    ("snet", 1),     // UpdateBobbingEnemy
    ("firb", 5),     // UpdateRollEnemy
    ("sbbb", 1),     // UpdateBobbingEnemy
    ("sir", 1),      // UpdateBobbingEnemy
    ("sbast", 1),    // UpdateBobbingEnemy
    ("fly", 5),      // UpdateRollEnemy
    ("asteroid", 6), // UpdateRoll2Enemy
];

/// `reactiveAnims[]` (name, animChar, animFrog, type), terminator (empty name, all zero) omitted.
pub const ORIG_REACTIVE_ANIMS: &[(&str, i16, i16, i32)] = &[
    ("null_fire.obe", -1, 4, 0),
    ("null_elec.obe", -1, 16, 16), // ANI_FACECAMERA
    ("electric.obe", -1, 5, 0),
    ("dobi.obe", -1, 0, 8), // ANI_ANTIFACE
    ("null_kick.obe", -1, 7, 0),
    ("null_pist.obe", -1, 3, 0),
    ("null_gib.obe", -1, 12, 0),
    ("NME.obe", -1, 7, 0),
    ("null_splat.obe", -1, 7, 0),
    ("moa.obe", -1, 12, 0),
    ("bee.obe", 1, 14, 0),
    ("moae.obe", -1, 12, 0),
    ("roll.obe", -1, 3, 8), // ANI_ANTIFACE
    ("roach.obe", -1, 9, 0),
    ("mummy.obe", -1, 7, 8), // ANI_ANTIFACE
    ("ancspike.obe", -1, 15, 0),
    ("ancspikea.obe", -1, 15, 0),
    ("spike.obe", -1, 10, 8), // ANI_ANTIFACE
    ("beetle.obe", -1, 9, 8), // ANI_ANTIFACE
    ("cball.obe", -1, 0, 8),  // ANI_ANTIFACE
    ("cnut.obe", -1, 6, 0),
    ("warthog.obe", -1, 1, 8), // ANI_ANTIFACE
    ("vult.obe", -1, 12, 8),   // ANI_ANTIFACE
    ("firb.obe", -1, 16, 0),
    ("imp.obe", -1, 13, 0),
    ("slug.obe", -1, 9, 0),
    ("antfly.obe", -1, 9, 0),
    ("louse.obe", -1, 10, 0),
    ("leech.obe", -1, 9, 0),
    ("lilant.obe", -1, 9, 0),
    ("sarge.obe", -1, 14, 0),
    ("antsnap.obe", -1, 9, 0),
    ("copter.obe", -1, 16, 0),
    ("ape.obe", -1, 11, 0),
    ("fly.obe", -1, 5, 0),
    ("stomp.obe", -1, 1, 0),
    ("vac.obe", -1, 14, 0),
    ("stomp.obe", -1, 3, 0),
    ("Labspy.obe", -1, 11, 0),
    ("smine.obe", -1, 11, 0),
    ("smineB.obe", -1, 11, 0),
    ("smineR.obe", -1, 11, 0),
    ("smineY.obe", -1, 11, 0),
    ("missle.obe", -1, 11, 0),
    ("missleC.obe", -1, 11, 0),
    ("missleR.obe", -1, 11, 0),
    ("missleY.obe", -1, 11, 0),
    ("missleb.obe", -1, 11, 0),
    ("misslebC.obe", -1, 11, 0),
    ("misslebR.obe", -1, 11, 0),
    ("misslebY.obe", -1, 11, 0),
    ("snet.obe", -1, 11, 0),
    ("sbast.obe", -1, 16, 0),
    ("sir.obe", -1, 16, 0),
    ("sbbb.obe", -1, 15, 0),
    ("knight.obe", -1, 10, 8), // ANI_ANTIFACE
    ("taran.obe", -1, 9, 0),
    ("hand.obe", -1, 12, 0),
    ("foot.obe", -1, 3, 8), // ANI_ANTIFACE
    ("pkin.obe", -1, 1, 1), // ANI_FACE
    ("ghostie.obe", -1, 8, 0),
    ("ghost.obe", -1, 7, 0),
    ("rat.obe", -1, 0, 8),      // ANI_ANTIFACE
    ("pistona.obe", -1, 3, 8),  // ANI_ANTIFACE
    ("pistonb.obe", -1, 3, 8),  // ANI_ANTIFACE
    ("pistonc.obe", -1, 3, 8),  // ANI_ANTIFACE
    ("pistond.obe", -1, 3, 8),  // ANI_ANTIFACE
    ("pistonaa.obe", -1, 3, 8), // ANI_ANTIFACE
    ("Grinder.obe", -1, 12, 0),
];
