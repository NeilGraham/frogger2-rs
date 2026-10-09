//! Path-following of platforms and enemies on small synthetic tile maps, created
//! through `MemLoadEntities` from hand-built entity definitions.

use std::sync::Arc;

use f2_formats::entity::{ActorDef, ActorKind, Entities, Item, Path as EPath, PathNode as ENode, Place};
use f2_game::assets::ModelInfo;
use f2_game::enemies::*;
use f2_game::ids::{EnemyId, PlatformId};
use f2_game::map::GameTile;
use f2_game::platform::*;
use f2_game::Game;
use f2_math::SVector;

/// A straight line of `n` tiles along +x, 100 units apart.
fn game_with_line(n: i16) -> Game {
    let mut g = Game::new();
    for i in 0..n {
        g.map.tiles.push(GameTile {
            centre: SVector::new(i * 100, 0, 0),
            normal: SVector::new(0, 4096, 0),
            dir_vector: [SVector::new(0, 0, 4096), SVector::new(0, 0, -4096), SVector::new(4096, 0, 0), SVector::new(-4096, 0, 0)],
            ..Default::default()
        });
    }
    let id = f2_formats::crc("box.obe");
    g.models.models.insert(id, Arc::new(ModelInfo { id, name: "box".into(), ..Default::default() }));
    g
}

fn node(tile: i32, speed: i32, wait: i32) -> ENode {
    ENode { place: Place::Tile(tile), offset: 0, offset2: 0, speed, wait_time: wait, sample_uid: 0 }
}

fn actor(kind: ActorKind, flags: u32, path_index: i16) -> Item {
    Item::Actor(ActorDef {
        kind,
        model: "box.obe".into(),
        flags: flags as i32,
        uid: 1,
        scale: 4096,
        radius: 0,
        anim_speed: 256,
        value1: 0,
        facing: 0,
        obj_flags: 0,
        depth_shift: 0,
        dff_clipping: 0,
        effects: 0,
        start_node: 0,
        path_index,
    })
}

fn load(g: &mut Game, paths: Vec<Vec<ENode>>, items: Vec<Item>) {
    let total_path_nodes = paths.iter().map(|p| p.len() as i16).sum();
    let count = |k: ActorKind| items.iter().filter(|i| matches!(i, Item::Actor(a) if a.kind == k)).count() as i16;
    let ents = Entities {
        fast: true,
        total_path_nodes,
        paths: paths.into_iter().map(|nodes| EPath { nodes }).collect(),
        total_enemies: count(ActorKind::Enemy),
        total_platforms: count(ActorKind::Platform),
        total_garibs: 0,
        total_camera_cases: 0,
        total_placeholders: count(ActorKind::Placeholder),
        items,
    };
    g.mem_load_entities(&ents);
}

fn plt_x(g: &Game, p: PlatformId) -> i16 {
    let a = g.platform.platform_list.array[p.idx()].plt_actor.unwrap();
    g.actor2.actors[a].actor.position.vx
}

fn nme_x(g: &Game, e: EnemyId) -> i16 {
    let a = g.enemies.enemy_list.array[e.idx()].nme_actor.unwrap();
    g.actor2.actors[a].actor.position.vx
}

/// Runs frames 1..=n of the platform/enemy updates, recording x of entity `f`.
fn run<T>(g: &mut Game, n: u32, f: impl Fn(&Game) -> T) -> Vec<T> {
    let mut xs = vec![f(g)];
    for frame in 1..=n {
        g.act_frame_count = frame;
        g.update_platforms();
        g.update_enemies();
        xs.push(f(g));
    }
    xs
}

#[test]
fn platform_ping_pong() {
    let mut g = game_with_line(3);
    // speed 4096 = one second (60 frames) per segment
    load(
        &mut g,
        vec![vec![node(0, 4096, 0), node(1, 4096, 0), node(2, 4096, 0)]],
        vec![actor(ActorKind::Platform, PLATFORM_NEW_FORWARDS | PLATFORM_NEW_PINGPONG, 0)],
    );
    let p = PlatformId(0);
    assert_eq!(g.platform.platform_list.array[0].update, Some(PlatformUpdate::Path));
    let xs = run(&mut g, 300, |g| plt_x(g, p));
    assert_eq!(xs[0], 0);
    assert_eq!(xs[30], 50);
    assert_eq!(xs[60], 100);
    assert_eq!(xs[90], 150);
    assert_eq!(xs[120], 200);
    assert_eq!(xs[150], 150); // coming back
    assert_eq!(xs[210], 50);
    assert_eq!(xs[240], 0);
    assert_eq!(xs[270], 50); // and out again
    assert!(g.platform.platform_list.array[0].flags & PLATFORM_NEW_FORWARDS != 0);
}

#[test]
fn platform_cycle_and_restart() {
    let mut g = game_with_line(3);
    let path = vec![node(0, 4096, 0), node(1, 4096, 0), node(2, 4096, 0)];
    load(
        &mut g,
        vec![path.clone(), path],
        vec![actor(ActorKind::Platform, PLATFORM_NEW_FORWARDS | PLATFORM_NEW_CYCLE, 0), actor(ActorKind::Platform, PLATFORM_NEW_FORWARDS, 1)],
    );
    // Same update function: the second platform is inserted before the first.
    let order = g.platform.platform_list.head.ids(&g.platform.platform_list.array);
    assert_eq!(order, vec![PlatformId(1), PlatformId(0)]);

    let xs = run(&mut g, 240, |g| (plt_x(g, PlatformId(0)), plt_x(g, PlatformId(1))));
    let cyc: Vec<i16> = xs.iter().map(|x| x.0).collect();
    let fwd: Vec<i16> = xs.iter().map(|x| x.1).collect();
    // 2 -> 0 is the closing segment of the cycle (frames 120..180).
    assert_eq!(cyc[150], 100);
    assert_eq!(cyc[180], 0);
    assert_eq!(cyc[210], 50);

    // Plain FORWARDS jumps back to node 0 after the last node.
    assert_eq!(fwd[119], 198);
    assert_eq!(fwd[120], 0);
    assert_eq!(fwd[150], 50);
}

#[test]
fn enemy_waits_at_nodes() {
    let mut g = game_with_line(3);
    // wait 2 at node 1 = 2 * waitScale(5) = 10 frames
    load(&mut g, vec![vec![node(0, 4096, 0), node(1, 4096, 2), node(2, 4096, 0)]], vec![actor(ActorKind::Enemy, ENEMY_NEW_FORWARDS | ENEMY_NEW_PINGPONG, 0)]);
    let e = EnemyId(0);
    assert_eq!(g.enemies.enemy_list.array[0].update[0], Some(EnemyUpdate::Path));
    assert!(g.enemies.enemy_list.array[0].flags & ENEMY_NEW_FOLLOWPATH != 0);
    let xs = run(&mut g, 400, |g| nme_x(g, e));
    assert_eq!(xs[30], 50);
    for (f, x) in xs.iter().enumerate().take(71).skip(60) {
        assert_eq!(*x, 100, "frame {f}: waiting at node 1");
    }
    assert_eq!(xs[71], 101);
    assert_eq!(xs[100], 150);
    // At a node with no wait the enemy is already one frame into the next segment
    // (`progress = max(actFrameCount - startFrame, 1)`).
    assert_eq!(xs[129], 198);
    assert_eq!(xs[130], 198);
    // ping-pong back: 2 -> 1 (no wait at 2), arriving at frame 190, wait 10 frames there again
    assert_eq!(xs[160], 150);
    assert_eq!(xs[190], 100);
    assert_eq!(xs[200], 100);
    assert_eq!(xs[230], 50);
    // inTile switches to the destination half-way through a segment.
    let tile_at = |g: &mut Game, f: u32| {
        g.act_frame_count = f;
        g.update_enemies();
        g.enemies.enemy_list.array[0].in_tile.map(|t| t.0)
    };
    // 0-60 0->1, wait to 70, 70-130 1->2, 130-190 2->1, wait to 200, 200-260 1->0,
    // 260-320 0->1, wait to 330, 330-390 1->2, 390-450 2->1.
    assert_eq!(tile_at(&mut g, 401), Some(2));
    assert_eq!(tile_at(&mut g, 425), Some(1));
}

#[test]
fn enemy_list_order_and_placeholders() {
    let mut g = game_with_line(3);
    let path = vec![node(0, 4096, 0), node(1, 4096, 0), node(2, 4096, 0)];
    load(
        &mut g,
        vec![path],
        vec![
            actor(ActorKind::Enemy, ENEMY_NEW_FORWARDS, 0),
            actor(ActorKind::Placeholder, 0, 0),
            actor(ActorKind::Enemy, ENEMY_NEW_BACKWARDS | ENEMY_NEW_CYCLE, 0),
        ],
    );
    let l = &g.enemies.enemy_list;
    assert_eq!(l.count, 3);
    // AddEnemy always inserts at the head (see BUG(original) note).
    assert_eq!(l.head.ids(&l.array), vec![EnemyId(2), EnemyId(1), EnemyId(0)]);
    assert_eq!(l.array[1].active, 0);
    assert!(l.array[1].nme_actor.is_none());
    // Both enemies share path 0's nodes (memload memcpy's the PATH).
    assert_eq!(l.array[0].path.nodes, l.array[2].path.nodes);
    // BACKWARDS from node 0 heads for the last node.
    assert_eq!((l.array[2].path.from_node, l.array[2].path.to_node), (0, 2));
    let xs = run(&mut g, 120, |g| nme_x(g, EnemyId(2)));
    assert_eq!(xs[30], 100);
    assert_eq!(xs[60], 198); // one frame into 2 -> 1
    assert_eq!(xs[90], 150);
}

#[test]
fn up_down_platform() {
    let mut g = game_with_line(1);
    let mut n = node(0, 4096, 0);
    n.offset2 = 60 << 12;
    load(&mut g, vec![vec![n]], vec![actor(ActorKind::Platform, PLATFORM_NEW_MOVEUP | PLATFORM_NEW_PINGPONG, 0)]);
    assert_eq!(g.platform.platform_list.array[0].update, Some(PlatformUpdate::UpDown));
    let p = PlatformId(0);
    let y = |g: &Game| {
        let a = g.platform.platform_list.array[0].plt_actor.unwrap();
        g.actor2.actors[a].actor.position.vy
    };
    let ys = run(&mut g, 200, y);
    assert_eq!(ys[30], 30);
    assert_eq!(ys[60], 60);
    assert_eq!(ys[90], 30); // flags inverted at the top: coming down
    assert!(
        g.platform.platform_list.array[p.idx()].flags & PLATFORM_NEW_MOVEUP != 0 || g.platform.platform_list.array[p.idx()].flags & PLATFORM_NEW_MOVEDOWN != 0
    );
}

#[test]
fn radius_check_box() {
    let a = SVector::new(0, 0, 0);
    assert!(radius_check(&a, &SVector::new(9, -9, 9), 10 << 12));
    assert!(!radius_check(&a, &SVector::new(10, 0, 0), 10 << 12));
}
