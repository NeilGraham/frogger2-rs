//! Toy deterministic simulation and a virtual-time match harness over the loopback transport.
#![allow(dead_code)]

use f2_net::*;

/// Integer "physics": four bodies pushed by pads, gravity, walls, body collisions and coins
/// spawned by an LCG RNG. Fully deterministic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toy {
    pub frame: u32,
    pub rng: u32,
    pub bodies: [[i32; 4]; 4], // x, y, vx, vy
    pub coins: Vec<(i32, i32)>,
    pub score: [i32; 4],
}

impl Default for Toy {
    fn default() -> Self {
        Toy { frame: 0, rng: 12345, bodies: [[1000, 0, 0, 0], [3000, 0, 0, 0], [5000, 0, 0, 0], [7000, 0, 0, 0]], coins: Vec::new(), score: [0; 4] }
    }
}

impl Toy {
    fn random(&mut self) -> u32 {
        self.rng = self.rng.wrapping_mul(1_103_515_245).wrapping_add(12345);
        (self.rng >> 16) & 0x7FFF
    }

    pub fn step(&mut self, inputs: PadInputs) {
        for (p, &pad) in inputs.iter().enumerate() {
            let b = &mut self.bodies[p];
            if pad & 1 != 0 {
                b[2] -= 3;
            }
            if pad & 2 != 0 {
                b[2] += 3;
            }
            if pad & 4 != 0 && b[1] == 0 {
                b[3] = 60;
            }
            b[3] -= 2; // gravity
            b[2] -= b[2] >> 4; // friction
            b[0] = b[0].wrapping_add(b[2]);
            b[1] = (b[1] + b[3]).max(0);
            if b[1] == 0 {
                b[3] = 0;
            }
            if !(0..=10_000).contains(&b[0]) {
                b[0] = b[0].clamp(0, 10_000);
                b[2] = -b[2];
            }
        }
        for i in 0..4 {
            for j in i + 1..4 {
                if (self.bodies[i][0] - self.bodies[j][0]).abs() < 100 && (self.bodies[i][1] - self.bodies[j][1]).abs() < 100 {
                    let (vi, vj) = (self.bodies[i][2], self.bodies[j][2]);
                    self.bodies[i][2] = vj;
                    self.bodies[j][2] = vi;
                }
            }
        }
        if self.random().is_multiple_of(32) && self.coins.len() < 16 {
            let x = (self.random() % 10_000) as i32;
            let y = (self.random() % 200) as i32;
            self.coins.push((x, y));
        }
        let bodies = self.bodies;
        let mut score = self.score;
        self.coins.retain(|&(cx, cy)| {
            for (p, b) in bodies.iter().enumerate() {
                if (b[0] - cx).abs() < 150 && (b[1] - cy).abs() < 150 {
                    score[p] += 1;
                    return false;
                }
            }
            true
        });
        self.score = score;
        self.frame += 1;
    }

    pub fn hash(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut mix = |v: u32| {
            for b in v.to_le_bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
        };
        mix(self.frame);
        mix(self.rng);
        for b in &self.bodies {
            for &v in b {
                mix(v as u32);
            }
        }
        mix(self.coins.len() as u32);
        for &(x, y) in &self.coins {
            mix(x as u32);
            mix(y as u32);
        }
        for &s in &self.score {
            mix(s as u32);
        }
        h
    }
}

impl Simulation for Toy {
    type Snapshot = Toy;
    fn tick(&mut self, inputs: PadInputs) {
        self.step(inputs);
    }
    fn snapshot(&self) -> Toy {
        self.clone()
    }
    fn restore(&mut self, s: &Toy) {
        *self = s.clone();
    }
    fn checksum(&self) -> u64 {
        self.hash()
    }
}

/// A deliberately nondeterministic sim: at frame 500 each instance adds its own hidden salt.
#[derive(Default)]
pub struct Flaky {
    pub inner: Toy,
    pub salt: i32,
}

impl Simulation for Flaky {
    type Snapshot = Toy;
    fn tick(&mut self, inputs: PadInputs) {
        if self.inner.frame == 500 {
            self.inner.bodies[0][0] += self.salt;
        }
        self.inner.step(inputs);
    }
    fn snapshot(&self) -> Toy {
        self.inner.clone()
    }
    fn restore(&mut self, s: &Toy) {
        self.inner = s.clone();
    }
    fn checksum(&self) -> u64 {
        self.inner.hash()
    }
}

/// Scripted pad for `slot` at `frame`: held for runs of varying length (realistic for a game,
/// and it makes predictions fail at every change).
pub fn input_for(slot: u8, frame: Frame) -> u16 {
    let mut x = (frame as u32 / 9).wrapping_mul(0x9E37_79B9) ^ (slot as u32).wrapping_mul(0x85EB_CA6B);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x & 0x1F) as u16
}

/// Single-process reference run with the same scripted inputs.
pub fn reference(num_players: u8, delay: u8, frames: Frame) -> u64 {
    let mut t = Toy::default();
    for f in 0..frames {
        let mut inputs = [0u16; 4];
        if f >= delay as Frame {
            for s in 0..num_players {
                inputs[s as usize] = input_for(s, f);
            }
        }
        t.tick(inputs);
    }
    t.hash()
}

pub fn frame_ms(step: u64) -> u64 {
    step * 1000 / 60
}

pub struct MatchReport {
    pub checksums: Vec<u64>,
    pub stats: Vec<NetStats>,
    pub events: Vec<Vec<NetEvent>>,
    pub spectator_checksum: Option<u64>,
    pub steps: u64,
}

/// Run a full match: lobby over the loopback network, then `frames` frames on every peer.
pub fn run_match(num_players: u8, config: SessionConfig, link: LoopbackConfig, frames: Frame, with_spectator: bool) -> MatchReport {
    let net = LoopbackNetwork::new(link);
    let host_t = net.endpoint();
    let host_addr = host_t.addr();
    let mut cfg = config.clone();
    cfg.num_players = num_players;
    let mut host = HostLobby::new(host_t, cfg, 0x5EED, with_spectator).unwrap();
    let mut joiners: Vec<Option<JoinLobby<LoopbackTransport>>> =
        (1..num_players).map(|_| Some(JoinLobby::new(net.endpoint(), host_addr, 0x5EED, false))).collect();
    let mut spectator_lobby = with_spectator.then(|| JoinLobby::new(net.endpoint(), host_addr, 0, true));

    // Lobby phase.
    let mut step = 0u64;
    loop {
        net.set_time(frame_ms(step));
        host.poll(frame_ms(step));
        for j in joiners.iter_mut().flatten() {
            assert!(!matches!(j.poll(frame_ms(step)), JoinStatus::Rejected(_)));
        }
        if let Some(s) = &mut spectator_lobby {
            s.poll(frame_ms(step));
        }
        step += 1;
        if host.num_players() == num_players && (!with_spectator || host.num_spectators() == 1) {
            break;
        }
        assert!(step < 6000, "lobby never filled");
    }

    let mut sessions: Vec<Option<(RollbackSession<Toy, LoopbackTransport>, Toy)>> = Vec::new();
    sessions.push(Some((host.start(frame_ms(step)).unwrap(), Toy::default())));
    for _ in 1..num_players {
        sessions.push(None);
    }
    let mut spectator: Option<(SpectatorSession<LoopbackTransport>, Toy)> = None;
    let mut events: Vec<Vec<NetEvent>> = vec![Vec::new(); num_players as usize];

    let limit = step + frames as u64 * 6 + 20_000;
    loop {
        let now = frame_ms(step);
        net.set_time(now);
        // Joiners waiting for Start.
        for (i, slot) in joiners.iter_mut().enumerate() {
            if let Some(j) = slot {
                if let JoinStatus::Started(info) = j.poll(now) {
                    let j = slot.take().unwrap();
                    let s = j.into_session::<Toy>(&info, now).unwrap();
                    assert_eq!(s.config().num_players, num_players);
                    sessions[i + 1] = Some((s, Toy::default()));
                }
            }
        }
        if let Some(l) = &mut spectator_lobby {
            if let JoinStatus::Started(info) = l.poll(now) {
                assert_eq!(info.local_slot, None);
                let l = spectator_lobby.take().unwrap();
                spectator = Some((l.into_spectator(&info, now), Toy::default()));
            }
        }
        for (i, entry) in sessions.iter_mut().enumerate() {
            let Some((s, sim)) = entry else { continue };
            s.poll(now);
            events[i].extend(s.events());
            if s.frame() >= frames || s.should_wait() {
                continue;
            }
            let slot = s.local_slot();
            let pad = input_for(slot, s.frame() + s.config().input_delay as Frame);
            match s.advance(sim, pad) {
                Ok(_) | Err(NetError::PredictionThreshold) => {}
                Err(e) => panic!("peer {i}: {e}"),
            }
        }
        if let Some((s, sim)) = &mut spectator {
            s.poll(now);
            // Catch up up to 3 frames per tick.
            for _ in 0..3 {
                if s.frame() >= frames || s.advance(sim).unwrap().is_none() {
                    break;
                }
            }
        }
        step += 1;
        let players_done = sessions.iter().all(|e| e.as_ref().is_some_and(|(s, _)| s.frame() >= frames && s.confirmed_frame() >= frames - 1));
        let spec_done = spectator.as_ref().is_none_or(|(s, _)| s.frame() >= frames) && spectator_lobby.is_none();
        if players_done && spec_done {
            break;
        }
        assert!(
            step < limit,
            "match did not finish: frames {:?}",
            sessions.iter().map(|e| e.as_ref().map(|(s, _)| (s.frame(), s.confirmed_frame()))).collect::<Vec<_>>()
        );
    }
    // A few more polls so trailing checksum reports get compared.
    for extra in 0..30 {
        let now = frame_ms(step + extra);
        net.set_time(now);
        for (i, entry) in sessions.iter_mut().enumerate() {
            let (s, sim) = entry.as_mut().unwrap();
            s.poll(now);
            s.settle(sim);
            events[i].extend(s.events());
        }
    }
    let mut checksums = Vec::new();
    let mut stats = Vec::new();
    for entry in &sessions {
        let (s, sim) = entry.as_ref().unwrap();
        checksums.push(sim.checksum());
        stats.push(s.stats());
    }
    MatchReport { checksums, stats, events, spectator_checksum: spectator.map(|(_, sim)| sim.checksum()), steps: step }
}
