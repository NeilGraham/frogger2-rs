//! The match finder of the optimal parser (`KrakenSuffixTrieMatcher`): a suffix trie over the
//! block that yields, per position, up to four (length, distance) pairs of strictly
//! decreasing distance (8 ints per position; a zero length ends the list).

/// Up to four (length, distance) pairs for every position of `data`.
pub(super) fn match_table(data: &[u8]) -> Vec<i32> {
    let mut matches = vec![0i32; data.len().max(1) * 8];
    Trie::new(data).run(&mut matches);
    matches
}

struct Trie<'a> {
    data: &'a [u8],
    size_plus1: i32,
    hash: Vec<i32>,
    node_pos: Vec<i32>,
    node_parent: Vec<i32>,
    node_follow: Vec<i32>,
    node_depth: Vec<i32>,
    child_start: Vec<i32>,
    child_count: Vec<i32>,
    child_capacity: Vec<i32>,
    child_char: Vec<u8>,
    child_node: Vec<i32>,
    child_used: i32,
    pos: i32,
    next_node: i32,
    last_ml: i32,
    cur: i32,
    node_to_update_follow: i32,
    maxml_offs: i32,
}

/// Where the walk of one position continues (the original's jump labels).
enum Step {
    /// Compare against the leaf at `-leaf` from `ml` on (`IL_03d9`).
    Leaf,
    /// Split internal node `node` (`IL_0437`).
    Split,
    /// Data exhausted: every later position repeats the last match (`IL_0471`).
    End,
    /// Record the match whose source is at `src` (`IL_04b3`).
    Record(i32),
}

impl<'a> Trie<'a> {
    fn new(data: &'a [u8]) -> Trie<'a> {
        let n = data.len() + 2;
        let child = if data.len() < 64 { 256 } else { data.len() * 2 };
        Trie {
            data,
            size_plus1: data.len() as i32 + 1,
            hash: vec![0; 65536],
            node_pos: vec![0; n],
            node_parent: vec![0; n],
            node_follow: vec![0; n],
            node_depth: vec![0; n],
            child_start: vec![0; n],
            child_count: vec![0; n],
            child_capacity: vec![0; n],
            child_char: vec![0; child],
            child_node: vec![0; child],
            child_used: 0,
            pos: 1,
            next_node: 1,
            last_ml: 0,
            cur: 0,
            node_to_update_follow: 0,
            maxml_offs: 0,
        }
    }

    fn d(&self, i: i32) -> u8 {
        self.data[i as usize]
    }

    fn run(&mut self, matches: &mut [i32]) {
        let length = self.data.len() as i32;
        let start = self.pos;
        let limit = (self.pos + length).min(self.size_plus1 - 4);
        let end = (self.pos + length).min(self.size_plus1);
        'positions: while self.pos < limit {
            if self.maxml_offs != 0 {
                self.fill_terminated(matches, start, limit);
                break;
            }
            let pos2 = self.pos;
            let remaining = self.size_plus1 - pos2;
            let min_ml = if self.last_ml > 1 { self.last_ml - 1 } else { 0 };
            let mut parent = 0; // num5
            let mut no_follow = 0; // num6
            let mut with_follow = 0; // num7
            let mut follow_limit = if self.node_to_update_follow == 0 {
                i32::MAX
            } else {
                let f = self.node_follow[self.node_to_update_follow as usize];
                if f != 0 {
                    self.node_depth[f as usize]
                } else {
                    0
                }
            };
            let mut node: i32; // num9
            let mut ml: i32; // num10
            let mut hash_slot = 0usize;
            let step: Step;
            'walk: {
                if self.cur < 1 {
                    hash_slot = ((self.d(pos2 - 1) as usize) | ((self.d(pos2) as usize) << 8)) & 0xffff;
                    node = self.hash[hash_slot];
                    if node == 0 {
                        self.hash[hash_slot] = -pos2;
                        self.last_ml = 0;
                        self.cur = 0;
                        matches[((pos2 - start) * 8) as usize] = 0;
                        self.pos = pos2 + 1;
                        continue 'positions;
                    }
                    ml = 2;
                    if node < 0 {
                        self.hash[hash_slot] = self.next_node;
                        step = Step::Leaf;
                        break 'walk;
                    }
                    let depth = self.node_depth[node as usize];
                    if ml < min_ml {
                        ml = min_ml;
                    }
                    if depth <= ml {
                        ml = depth;
                    } else {
                        let other = self.node_pos[node as usize];
                        while ml < depth {
                            if ml == remaining {
                                step = Step::End;
                                break 'walk;
                            }
                            if self.d(pos2 - 1 + ml) == self.d(other - 1 + ml) {
                                ml += 1;
                            } else {
                                self.hash[hash_slot] = self.next_node;
                                step = Step::Split;
                                break 'walk;
                            }
                        }
                    }
                } else {
                    node = self.cur;
                    ml = self.node_depth[node as usize];
                }
                // Descend.
                loop {
                    if follow_limit < ml {
                        let u = self.node_to_update_follow as usize;
                        if ml < self.node_depth[u] {
                            follow_limit = ml;
                            self.node_follow[u] = node;
                        } else {
                            follow_limit = i32::MAX;
                        }
                    }
                    if ml >= remaining {
                        step = Step::End;
                        break 'walk;
                    }
                    let c = self.d(pos2 - 1 + ml);
                    if self.node_follow[node as usize] == 0 {
                        no_follow = node;
                    } else {
                        with_follow = node;
                    }
                    let Some(slot) = self.find_child_slot(node, c) else {
                        self.add_child(node, c, -pos2);
                        self.node_to_update_follow = if no_follow > 0 { no_follow } else { with_follow };
                        let src = self.node_pos[node as usize];
                        parent = node;
                        step = Step::Record(src);
                        break 'walk;
                    };
                    let child = self.child_node[slot];
                    if child < 0 {
                        // A leaf.
                        self.child_node[slot] = self.next_node;
                        parent = node;
                        ml += 1;
                        node = child;
                        step = Step::Leaf;
                        break 'walk;
                    }
                    ml += 1;
                    let depth = self.node_depth[child as usize];
                    if ml < min_ml {
                        ml = min_ml;
                    }
                    if ml < depth {
                        let other = self.node_pos[child as usize];
                        while ml < depth {
                            if ml == remaining {
                                node = child;
                                step = Step::End;
                                break 'walk;
                            }
                            if self.d(pos2 - 1 + ml) == self.d(other - 1 + ml) {
                                ml += 1;
                            } else {
                                self.child_node[slot] = self.next_node;
                                parent = node;
                                node = child;
                                step = Step::Split;
                                break 'walk;
                            }
                        }
                    } else {
                        ml = depth;
                    }
                    node = child;
                }
            }
            let _ = hash_slot;
            let src = match step {
                Step::Leaf => {
                    let leaf = -node;
                    if ml < min_ml {
                        ml = min_ml;
                    }
                    let mut split = false;
                    while ml != remaining {
                        if self.d(pos2 - 1 + ml) == self.d(leaf - 1 + ml) {
                            ml += 1;
                        } else {
                            split = true;
                            break;
                        }
                    }
                    if !split {
                        self.end(matches, pos2, node, remaining, start, limit);
                        break;
                    }
                    self.make_node(pos2, ml, parent, node, leaf, &mut follow_limit)
                }
                Step::Split => {
                    let other = self.node_pos[node as usize];
                    self.node_parent[node as usize] = self.next_node;
                    self.make_node(pos2, ml, parent, node, other, &mut follow_limit)
                }
                Step::End => {
                    self.end(matches, pos2, node, remaining, start, limit);
                    break;
                }
                Step::Record(src) => src,
            };
            // Record the matches of this position.
            self.cur = if with_follow != 0 { self.node_follow[with_follow as usize] } else { 0 };
            self.last_ml = ml;
            let base = ((pos2 - start) * 8) as usize;
            matches[base] = ml;
            matches[base + 1] = pos2 - src;
            let mut pairs = 1usize;
            let mut walked = 0;
            let mut n = parent;
            while n != 0 {
                if pairs < 4 {
                    let dist = pos2 - self.node_pos[n as usize];
                    if dist < matches[base + (pairs - 1) * 2 + 1] {
                        let depth = self.node_depth[n as usize];
                        if depth < 3 {
                            break;
                        }
                        matches[base + pairs * 2] = depth;
                        matches[base + pairs * 2 + 1] = dist;
                        pairs += 1;
                    }
                }
                walked += 1;
                if walked > 16 {
                    break;
                }
                self.node_pos[n as usize] = pos2;
                n = self.node_parent[n as usize];
            }
            if pairs < 4 {
                matches[base + pairs * 2] = 0;
            }
            self.pos = pos2 + 1;
        }
        for i in self.pos..end {
            matches[((i - start) * 8) as usize] = 0;
        }
        self.pos = end;
    }

    /// The rest of the data repeats an earlier position (`IL_0471`).
    fn end(&mut self, matches: &mut [i32], pos2: i32, node: i32, remaining: i32, start: i32, limit: i32) {
        let src = if node < 0 { -node } else { self.node_pos[node as usize] };
        self.maxml_offs = pos2 - src;
        self.last_ml = remaining;
        self.fill_terminated(matches, start, limit);
        self.pos = limit;
    }

    fn fill_terminated(&mut self, matches: &mut [i32], start: i32, process_end: i32) {
        for i in self.pos..process_end {
            let b = ((i - start) * 8) as usize;
            matches[b] = self.size_plus1 - i;
            matches[b + 1] = self.maxml_offs;
            matches[b + 2] = 0;
        }
        self.pos = process_end;
    }

    fn make_node(&mut self, pos: i32, curml: i32, parent: i32, cur: i32, other_pos: i32, follow_limit: &mut i32) -> i32 {
        let num = self.next_node;
        self.next_node += 1;
        let u = num as usize;
        self.node_depth[u] = curml;
        self.node_follow[u] = 0;
        self.node_pos[u] = pos;
        self.node_parent[u] = parent;
        self.child_start[u] = 0;
        self.child_count[u] = 0;
        self.child_capacity[u] = 0;
        self.add_child(num, self.d(pos - 1 + curml), -pos);
        self.add_child(num, self.d(other_pos - 1 + curml), cur);
        if *follow_limit < curml {
            let f = self.node_to_update_follow as usize;
            if curml < self.node_depth[f] {
                *follow_limit = curml;
                self.node_follow[f] = num;
            } else {
                *follow_limit = i32::MAX;
            }
        }
        self.node_to_update_follow = num;
        other_pos
    }

    fn find_child_slot(&self, node: i32, c: u8) -> Option<usize> {
        let start = self.child_start[node as usize] as usize;
        let count = self.child_count[node as usize] as usize;
        (start..start + count).find(|&i| self.child_char[i] == c)
    }

    fn add_child(&mut self, node: i32, c: u8, value: i32) {
        let u = node as usize;
        let count = self.child_count[u];
        let cap = self.child_capacity[u];
        if count == cap {
            let new_cap = if cap == 0 { 2 } else { cap * 2 };
            let needed = (self.child_used + new_cap) as usize;
            if needed > self.child_char.len() {
                let mut size = self.child_char.len();
                while size < needed {
                    size *= 2;
                }
                self.child_char.resize(size, 0);
                self.child_node.resize(size, 0);
            }
            let at = self.child_used;
            self.child_used += new_cap;
            if count > 0 {
                let s = self.child_start[u] as usize;
                self.child_char.copy_within(s..s + count as usize, at as usize);
                self.child_node.copy_within(s..s + count as usize, at as usize);
            }
            self.child_start[u] = at;
            self.child_capacity[u] = new_cap;
        }
        let slot = (self.child_start[u] + count) as usize;
        self.child_char[slot] = c;
        self.child_node[slot] = value;
        self.child_count[u] = count + 1;
    }
}
