//! Port of `Common/path.c` / `path.h`: enemy/platform paths over tiles.
//!
//! On PC every entity's nodes come from one block per level (`AllocatePathList` +
//! `GetPathNodes`, freed by `FreePathList`). The port keeps that block as
//! `PathState::nodes`; a [`Path`] stores the index of its first node. `MemLoadEntities`
//! `memcpy`s a `PATH` per entity, so entities created from the same path index share
//! their nodes (enemy code writes `nodes[n].worldTile` at runtime) - the pool keeps
//! that aliasing.

use crate::prelude::*;

use f2_math::{fmul, make_unit, sub_vector_fss, FVector, Fixed, Rng, SVector};
use serde::{Deserialize, Serialize};

use crate::effects::Effect;
use crate::ids::TileId;
use crate::map::Map;

/// `PATH_MAKENODETILEPTRS` (`path.h`).
pub const PATH_MAKENODETILEPTRS: u32 = 1 << 0;

/// `PATHNODE` (`path.h`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathNode {
    /// `worldTile`.
    pub world_tile: Option<TileId>,
    pub offset: Fixed,
    pub offset2: Fixed,
    pub speed: Fixed,
    pub wait_time: i16,
    /// `sample`: sample uid (0 = `NULL`), resolved by the audio layer.
    pub sample: u32,
}

/// `PATH` (`path.h`). `nodes` is the index of node 0 in `PathState::nodes`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Path {
    pub num_nodes: i16,
    pub from_node: i16,
    pub to_node: i16,
    pub start_node: i16,
    pub start_frame: u32,
    pub end_frame: u32,
    pub nodes: u32,
}

impl Path {
    /// `GET_PATHLASTNODE(x)`.
    #[inline]
    pub fn last_node(&self) -> i16 {
        self.num_nodes - 1
    }

    /// Pool index of `nodes[i]`.
    #[inline]
    pub fn node_index(&self, i: i32) -> usize {
        (self.nodes as i64 + i as i64) as usize
    }
}

/// Globals of `path.c` (`pathCount`, `pathAlloc`, and the `pathNodes` block).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathState {
    pub path_count: u16,
    pub path_alloc: u16,
    /// The node block (`pathNodes`).
    pub nodes: Vec<PathNode>,
    /// Phase 3S3: changes whenever the node block may have changed (a cache key for
    /// [`NodePosCache`]; not state).
    #[serde(skip)]
    pub(crate) gen: NodeGen,
}

/// Phase 3S3: a counter of writes to [`PathState::nodes`] (every writer goes through
/// [`PathState::touch`]); compares equal whatever it holds (not state).
#[derive(Clone, Copy, Default)]
pub struct NodeGen(u32);

impl PartialEq for NodeGen {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for NodeGen {}

impl core::fmt::Debug for NodeGen {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("NodeGen")
    }
}

/// Phase 3S3: `GetPositionForPathNode` of an entity's `fromNode` and `toNode` as last
/// computed, for the node block generation and node indices in the key. Node positions
/// depend only on the node (tile, offset) and on the tile's centre and normal, which never
/// change after the level load. Not state (never serialized, compared or printed).
#[derive(Clone, Copy, Default)]
pub struct NodePosCache {
    valid: bool,
    gen: u32,
    nodes: u32,
    from: i16,
    to: i16,
    from_pos: SVector,
    to_pos: SVector,
}

impl PartialEq for NodePosCache {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for NodePosCache {}

impl core::fmt::Debug for NodePosCache {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("NodePosCache")
    }
}

impl NodePosCache {
    #[inline]
    pub(crate) fn matches(&self, gen: u32, path: &Path) -> bool {
        self.valid && self.gen == gen && self.nodes == path.nodes && self.from == path.from_node && self.to == path.to_node
    }

    /// `(to, from)` positions.
    #[inline]
    pub(crate) fn get(&self) -> (SVector, SVector) {
        (self.to_pos, self.from_pos)
    }

    #[inline]
    pub(crate) fn set(&mut self, gen: u32, path: &Path, to: SVector, from: SVector) {
        *self = NodePosCache { valid: true, gen, nodes: path.nodes, from: path.from_node, to: path.to_node, from_pos: from, to_pos: to };
    }
}

impl PathState {
    /// The node block generation ([`NodePosCache`] key).
    #[inline]
    pub(crate) fn gen_value(&self) -> u32 {
        self.gen.0
    }

    /// Marks the node block as (possibly) changed.
    #[inline]
    pub(crate) fn touch(&mut self) {
        self.gen.0 = self.gen.0.wrapping_add(1);
    }

    /// Port of `AllocatePathList` (Common/path.c:96).
    pub fn allocate_path_list(&mut self, num_nodes: i32) {
        self.touch();
        self.path_count = 0;
        self.path_alloc = num_nodes as u16;
        self.nodes = Vec::with_capacity(num_nodes.max(0) as usize);
    }

    /// Port of `GetPathNodes` (Common/path.c:113): appends `num` zeroed nodes to the
    /// block and returns the index of the first.
    pub fn get_path_nodes(&mut self, num_nodes: i32, effects: &mut Vec<Effect>) -> u32 {
        if self.path_count as i32 + num_nodes > self.path_alloc as i32 {
            effects.push(Effect::Log {
                text: format!("Error! Tried to allocate {} nodes with {} left", num_nodes, self.path_alloc as i32 - self.path_count as i32),
            });
        }
        self.touch();
        self.path_count = self.path_count.wrapping_add(num_nodes as u16);
        let first = self.nodes.len() as u32;
        self.nodes.resize(self.nodes.len() + num_nodes.max(0) as usize, PathNode::default());
        first
    }

    /// The nodes of `path` (`path->nodes[0..numNodes]`).
    pub fn nodes(&self, path: &Path) -> &[PathNode] {
        let a = path.nodes as usize;
        &self.nodes[a..a + path.num_nodes.max(0) as usize]
    }

    /// Mutable nodes of `path`.
    pub fn nodes_mut(&mut self, path: &Path) -> &mut [PathNode] {
        self.touch();
        let a = path.nodes as usize;
        &mut self.nodes[a..a + path.num_nodes.max(0) as usize]
    }

    /// `&path->nodes[i]`.
    pub fn node(&self, path: &Path, i: i32) -> &PathNode {
        &self.nodes[path.node_index(i)]
    }

    /// `&path->nodes[i]` (mutable).
    pub fn node_mut(&mut self, path: &Path, i: i32) -> &mut PathNode {
        self.touch();
        &mut self.nodes[path.node_index(i)]
    }

    /// Port of `FreePathList` (Common/path.c:65).
    pub fn free_path_list(&mut self) {
        let gen = self.gen;
        *self = PathState::default();
        // (The counter continues: caches of the old block must not match the new one.)
        self.gen = gen;
        self.touch();
    }
}

#[inline]
fn node_pos(map: &Map, node: &PathNode, offset: Fixed) -> SVector {
    let t = map.tile(node.world_tile.expect("path node without tile"));
    let c = |centre: i16, n: i16| -> i16 { (centre as i32).wrapping_add((fmul(n as i32, offset) >> 12) as i16 as i32) as i16 };
    SVector { vx: c(t.centre.vx, t.normal.vx), vy: c(t.centre.vy, t.normal.vy), vz: c(t.centre.vz, t.normal.vz), pad: 0 }
}

/// Port of `GetPositionForPathNode` (Common/path.c:141). The `pad` of the
/// destination is left untouched by the original; callers keep their own.
pub fn get_position_for_path_node(map: &Map, node: &PathNode) -> SVector {
    node_pos(map, node, node.offset)
}

/// Port of `GetPositionForPathNodeOffset2` (Common/path.c:156).
pub fn get_position_for_path_node_offset2(map: &Map, node: &PathNode) -> SVector {
    node_pos(map, node, node.offset2)
}

/// Port of `ScalePathSpeed` (Common/path.c:179).
pub fn scale_path_speed(nodes: &mut [PathNode], scale: Fixed) {
    for n in nodes.iter_mut() {
        n.speed = fmul(n.speed, scale);
    }
}

/// Port of `SetAllPathNodesToSetSpeed` (Common/path.c:195).
pub fn set_all_path_nodes_to_set_speed(nodes: &mut [PathNode], speed: Fixed) {
    for n in nodes.iter_mut() {
        n.speed = speed;
    }
}

/// Port of `SetAllPathNodesToSetWaitTime` (Common/path.c:210).
pub fn set_all_path_nodes_to_set_wait_time(nodes: &mut [PathNode], pause: i16) {
    for n in nodes.iter_mut() {
        n.wait_time = pause;
    }
}

/// Port of `CalcDirectionVectorFromNodeToNode` (Common/path.c:225).
pub fn calc_direction_vector_from_node_to_node(map: &Map, from: &PathNode, to: &PathNode) -> FVector {
    let vec_from = get_position_for_path_node(map, from);
    let vec_to = get_position_for_path_node(map, to);
    let mut result = FVector::default();
    sub_vector_fss(&mut result, &vec_to, &vec_from);
    make_unit(&mut result);
    result
}

/// Port of `AssignRandomSpeedsToPathNodes` (Common/path.c:243). Nodes are visited
/// last to first (`while(i--)`), which fixes the `Random` call order.
pub fn assign_random_speeds_to_path_nodes(rng: &mut Rng, nodes: &mut [PathNode], min_speed: Fixed, max_speed: Fixed) {
    let mut i = nodes.len() as i32;
    while i > 0 {
        i -= 1;
        nodes[i as usize].speed = rng.random(max_speed as u32).wrapping_add(min_speed);
    }
}

/// Port of `AssignRandomWaitTimesToPathNodes` (Common/path.c:258).
pub fn assign_random_wait_times_to_path_nodes(rng: &mut Rng, nodes: &mut [PathNode], min_pause: i16, max_pause: Fixed) {
    let mut i = nodes.len() as i32;
    while i > 0 {
        i -= 1;
        nodes[i as usize].wait_time = rng.random(max_pause as u32).wrapping_add(min_pause as i32) as i16;
    }
}

/// Port of `CheckPathOrientation` (Common/path.c:267): 1 when every node except the
/// last sits on a tile with the first node's normal.
pub fn check_path_orientation(map: &Map, nodes: &[PathNode]) -> i32 {
    let first = map.tile(nodes[0].world_tile.expect("path node without tile")).normal;
    for node in nodes.iter().take(nodes.len().saturating_sub(1)) {
        let n = map.tile(node.world_tile.expect("path node without tile")).normal;
        if n.vx != first.vx || n.vy != first.vy || n.vz != first.vz {
            return 0;
        }
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::GameTile;
    use f2_math::make_svector;

    fn map() -> Map {
        let mut m = Map::default();
        m.tiles.push(GameTile { centre: make_svector(100, 0, 0), normal: make_svector(0, 4096, 0), ..Default::default() });
        m.tiles.push(GameTile { centre: make_svector(100, 0, 500), normal: make_svector(0, 4096, 0), ..Default::default() });
        m.tiles.push(GameTile { centre: make_svector(0, 0, 0), normal: make_svector(4096, 0, 0), ..Default::default() });
        m
    }

    fn node(t: u32, offset: Fixed) -> PathNode {
        PathNode { world_tile: Some(TileId(t)), offset, offset2: -offset, speed: 4096, ..Default::default() }
    }

    #[test]
    fn positions() {
        let m = map();
        // offset is fixed-point world units: 50.0 along the normal.
        let p = get_position_for_path_node(&m, &node(0, 50 << 12));
        assert_eq!((p.vx, p.vy, p.vz), (100, 50, 0));
        let p = get_position_for_path_node_offset2(&m, &node(0, 50 << 12));
        assert_eq!(p.vy, -50);
        let d = calc_direction_vector_from_node_to_node(&m, &node(0, 0), &node(1, 0));
        assert_eq!((d.vx, d.vy), (0, 0));
        assert!((4090..=4096).contains(&d.vz), "{d:?}"); // MakeUnit is approximate
    }

    #[test]
    fn speeds_and_orientation() {
        let m = map();
        let mut p = vec![node(0, 0), node(1, 0), node(2, 0)];
        assert_eq!(check_path_orientation(&m, &p), 1); // last node not checked
        p.swap(1, 2);
        assert_eq!(check_path_orientation(&m, &p), 0);
        scale_path_speed(&mut p, 2048);
        assert!(p.iter().all(|n| n.speed == 2048));
        set_all_path_nodes_to_set_wait_time(&mut p, 7);
        assert!(p.iter().all(|n| n.wait_time == 7));
        let mut rng = Rng::default();
        rng.seed_random();
        let mut rng2 = rng;
        assign_random_speeds_to_path_nodes(&mut rng, &mut p, 10, 100);
        let expect: Vec<i32> = (0..3).map(|_| rng2.random(100) + 10).collect();
        assert_eq!(p[2].speed, expect[0]);
        assert_eq!(p[0].speed, expect[2]);
    }

    #[test]
    fn block_counters() {
        let mut s = PathState::default();
        let mut fx = Vec::new();
        s.allocate_path_list(4);
        assert_eq!(s.get_path_nodes(3, &mut fx), 0);
        assert_eq!(s.nodes.len(), 3);
        assert!(fx.is_empty());
        s.get_path_nodes(2, &mut fx);
        assert_eq!(fx.len(), 1);
    }
}
