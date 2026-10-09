//! End-to-end netplay tests over the in-memory loopback network (virtual time) and real UDP.

mod common;

use common::*;
use f2_net::*;

const FRAMES: Frame = 10_000;

fn lossy(latency_ms: u64, seed: u64) -> LoopbackConfig {
    LoopbackConfig { latency_ms, jitter_ms: if latency_ms == 0 { 5 } else { latency_ms / 4 }, loss: 0.05, duplicate: 0.01, seed }
}

fn check_match(players: u8, latency: u64, mode: SyncMode, frames: Frame, input_delay: u8) -> MatchReport {
    let config = SessionConfig { mode, input_delay, max_prediction: 8, ..SessionConfig::default() };
    let r = run_match(players, config.clone(), lossy(latency, 0xC0FFEE + latency + players as u64), frames, false);
    let expect = reference(players, config.input_delay, frames);
    for (i, s) in r.stats.iter().enumerate() {
        println!(
            "{mode:?} players={players} latency={latency}ms peer{i}: rollbacks={} resimulated={} max_depth={} stalls={} waits={} rtt={:?} sent={} recv={} dropped={} checksums={}",
            s.rollbacks, s.rollback_frames, s.max_rollback, s.stalls, s.wait_frames, s.rtt_ms,
            s.packets_sent, s.packets_received, s.packets_dropped, s.checksums_compared
        );
    }
    println!("finished after {} ticks for {frames} frames", r.steps);
    for (i, &c) in r.checksums.iter().enumerate() {
        assert_eq!(c, expect, "peer {i} diverged from the reference run");
    }
    for (i, ev) in r.events.iter().enumerate() {
        assert!(!ev.iter().any(|e| matches!(e, NetEvent::Desync { .. } | NetEvent::Disconnected { .. })), "peer {i}: {ev:?}");
    }
    for s in &r.stats {
        assert!(s.checksums_compared > 0);
        assert_eq!(s.desyncs, 0);
        if mode == SyncMode::Lockstep {
            assert_eq!(s.rollbacks, 0);
        }
    }
    r
}

macro_rules! rollback_test {
    ($name:ident, $players:expr, $latency:expr) => {
        #[test]
        fn $name() {
            let r = check_match($players, $latency, SyncMode::Rollback, FRAMES, 2);
            if $latency >= 50 {
                assert!(r.stats.iter().all(|s| s.rollbacks > 0), "expected rollbacks at {}ms", $latency);
            }
        }
    };
}

rollback_test!(rollback_2p_0ms, 2, 0);
rollback_test!(rollback_2p_50ms, 2, 50);
rollback_test!(rollback_2p_150ms, 2, 150);
rollback_test!(rollback_3p_0ms, 3, 0);
rollback_test!(rollback_3p_50ms, 3, 50);
rollback_test!(rollback_3p_150ms, 3, 150);
rollback_test!(rollback_4p_0ms, 4, 0);
rollback_test!(rollback_4p_50ms, 4, 50);
rollback_test!(rollback_4p_150ms, 4, 150);

/// Lockstep runs near real time once `input_delay` covers the one-way latency + jitter.
#[test]
fn lockstep_2p_50ms() {
    let r = check_match(2, 50, SyncMode::Lockstep, FRAMES, 5);
    assert!(r.steps < FRAMES as u64 * 13 / 10, "lockstep too slow: {} ticks", r.steps);
}

#[test]
fn lockstep_4p_150ms() {
    check_match(4, 150, SyncMode::Lockstep, FRAMES, 13);
}

/// With too little delay lockstep still stays in sync, it just runs slower than real time.
#[test]
fn lockstep_low_delay_is_slow_but_correct() {
    let r = check_match(2, 50, SyncMode::Lockstep, 2000, 1);
    assert!(r.steps > 3000);
}

#[test]
fn spectator_follows_host() {
    let config = SessionConfig::default();
    let r = run_match(3, config.clone(), lossy(40, 99), 3000, true);
    let expect = reference(3, config.input_delay, 3000);
    assert!(r.checksums.iter().all(|&c| c == expect));
    assert_eq!(r.spectator_checksum, Some(expect));
}

/// Two peers built directly (no lobby) on a loopback network.
fn direct_pair<S: Simulation>(net: &LoopbackNetwork, config: &SessionConfig) -> [RollbackSession<S, LoopbackTransport>; 2] {
    let a = net.endpoint();
    let b = net.endpoint();
    let (aa, ba) = (a.addr(), b.addr());
    [
        RollbackSession::new(a, config.clone(), 0, 7, vec![(PeerKind::Player(1), ba)], 0).unwrap(),
        RollbackSession::new(b, config.clone(), 1, 7, vec![(PeerKind::Player(0), aa)], 0).unwrap(),
    ]
}

#[test]
fn desync_is_detected() {
    let net = LoopbackNetwork::new(lossy(30, 5));
    let config = SessionConfig { checksum_interval: 30, ..SessionConfig::default() };
    let mut sessions = direct_pair::<Flaky>(&net, &config);
    let mut sims = [Flaky { salt: 1, ..Default::default() }, Flaky { salt: 2, ..Default::default() }];
    let mut desyncs = [Vec::new(), Vec::new()];
    for step in 0..3000u64 {
        net.set_time(frame_ms(step));
        for i in 0..2 {
            let s = &mut sessions[i];
            s.poll(frame_ms(step));
            for e in s.events() {
                if let NetEvent::Desync { frame, .. } = e {
                    desyncs[i].push(frame);
                }
            }
            if !s.should_wait() {
                let pad = input_for(i as u8, s.frame() + 2);
                let _ = s.advance(&mut sims[i], pad);
            }
        }
    }
    for (i, d) in desyncs.iter().enumerate() {
        println!("peer {i}: first desync at frame {:?}, {} reports", d.first(), d.len());
        assert!(!d.is_empty(), "peer {i} did not detect the desync");
        // Checksums before the divergence matched; the first mismatch is the first checksum
        // frame after 500.
        assert_eq!(*d.iter().min().unwrap(), 510);
    }
}

#[test]
fn deterministic_sims_report_no_desync_and_matching_checksums() {
    let net = LoopbackNetwork::new(lossy(30, 6));
    let config = SessionConfig::default();
    let mut sessions = direct_pair::<Toy>(&net, &config);
    let mut sims = [Toy::default(), Toy::default()];
    for step in 0..2000u64 {
        net.set_time(frame_ms(step));
        for i in 0..2 {
            let s = &mut sessions[i];
            s.poll(frame_ms(step));
            assert!(s.events().iter().all(|e| !matches!(e, NetEvent::Desync { .. })));
            if !s.should_wait() {
                let pad = input_for(i as u8, s.frame() + 2);
                let _ = s.advance(&mut sims[i], pad);
            }
        }
    }
    let (a, b) = (sessions[0].latest_checksum().unwrap(), sessions[1].latest_checksum().unwrap());
    assert!(a.0 > 1800 && b.0 > 1800);
    assert!(sessions[0].stats().checksums_compared > 25);
}

#[test]
fn time_sync_slows_the_peer_that_is_ahead() {
    let net = LoopbackNetwork::new(LoopbackConfig { latency_ms: 30, ..Default::default() });
    let config = SessionConfig { max_prediction: 20, ..SessionConfig::default() };
    let mut sessions = direct_pair::<Toy>(&net, &config);
    let mut sims = [Toy::default(), Toy::default()];
    // Peer 1 starts 15 frames late.
    for step in 0..1500u64 {
        net.set_time(frame_ms(step));
        for i in 0..2 {
            if i == 1 && step < 15 {
                continue;
            }
            let s = &mut sessions[i];
            s.poll(frame_ms(step));
            if !s.should_wait() {
                let pad = input_for(i as u8, s.frame() + 2);
                let _ = s.advance(&mut sims[i], pad);
            }
        }
    }
    let (s0, s1) = (sessions[0].stats(), sessions[1].stats());
    let gap = sessions[0].frame() - sessions[1].frame();
    println!("waits: {} / {}, final frames {} / {} (gap {gap})", s0.wait_frames, s1.wait_frames, sessions[0].frame(), sessions[1].frame());
    assert!(s0.wait_frames >= 8, "early peer should have waited");
    assert!(gap.abs() <= 3, "peers should converge, gap {gap}");
    assert!(s0.rtt_ms[1].is_some_and(|r| (50..=75).contains(&r)), "rtt {:?}", s0.rtt_ms);
}

#[test]
fn lobby_rejects_when_full() {
    let net = LoopbackNetwork::new(LoopbackConfig::default());
    let host_t = net.endpoint();
    let host_addr = host_t.addr();
    let config = SessionConfig { num_players: 2, ..SessionConfig::default() };
    let mut host = HostLobby::new(host_t, config, 0x1234, false).unwrap();
    let mut j1 = JoinLobby::new(net.endpoint(), host_addr, 0x1234, false);
    let mut j2 = JoinLobby::new(net.endpoint(), host_addr, 0, false);
    let mut wrong = JoinLobby::new(net.endpoint(), host_addr, 0x9999, false);
    let mut welcomed = false;
    let mut rejected = false;
    for step in 0..20 {
        assert!(matches!(wrong.poll(step), JoinStatus::Waiting));
        if let JoinStatus::Welcomed { slot } = j1.poll(step) {
            assert_eq!(slot, Some(1));
            welcomed = true;
        }
        host.poll(step);
        if welcomed {
            if let JoinStatus::Rejected(r) = j2.poll(step) {
                assert_eq!(r, wire::REJECT_FULL);
                rejected = true;
            }
        }
    }
    assert!(welcomed && rejected);
    assert_eq!(host.num_players(), 2);
}

#[test]
fn udp_two_players_with_join_code() {
    use std::time::{Duration, Instant};
    let host_t = UdpTransport::bind("127.0.0.1:0").unwrap();
    let addr = host_t.local_addr().unwrap();
    let nonce = joincode::random_nonce();
    let code = encode_join_code(addr, nonce);
    let config = SessionConfig::default();
    let mut host = HostLobby::new(host_t, config.clone(), nonce, false).unwrap();

    let (host_addr, code_nonce) = decode_join_code(&code).unwrap();
    let mut joiner = Some(JoinLobby::new(UdpTransport::bind("127.0.0.1:0").unwrap(), host_addr, code_nonce, false));

    let t0 = Instant::now();
    let now = || t0.elapsed().as_millis() as u64;
    let mut joined: Option<RollbackSession<Toy, UdpTransport>> = None;
    while joined.is_none() {
        host.poll(now());
        if let JoinStatus::Started(info) = joiner.as_mut().unwrap().poll(now()) {
            joined = Some(joiner.take().unwrap().into_session(&info, now()).unwrap());
        }
        if host.num_players() == 2 && joiner.is_some() {
            // Host starts as soon as the second player is in; the joiner sees Start next poll.
            break;
        }
        assert!(now() < 5000);
        std::thread::sleep(Duration::from_millis(1));
    }
    let mut sessions: Vec<Option<RollbackSession<Toy, UdpTransport>>> = vec![Some(host.start(now()).unwrap()), joined];
    let mut sims = [Toy::default(), Toy::default()];
    let frames = 600;
    let mut next_tick = Instant::now();
    loop {
        if sessions[1].is_none() {
            if let JoinStatus::Started(info) = joiner.as_mut().unwrap().poll(now()) {
                sessions[1] = Some(joiner.take().unwrap().into_session(&info, now()).unwrap());
            }
        }
        for i in 0..2 {
            let Some(s) = sessions[i].as_mut() else { continue };
            s.poll(now());
            if s.frame() < frames && !s.should_wait() {
                let pad = input_for(i as u8, s.frame() + 2);
                match s.advance(&mut sims[i], pad) {
                    Ok(_) | Err(NetError::PredictionThreshold) => {}
                    Err(e) => panic!("{e}"),
                }
            }
        }
        if sessions.iter().all(|s| s.as_ref().is_some_and(|s| s.frame() >= frames && s.confirmed_frame() >= frames - 1)) {
            break;
        }
        assert!(now() < 30_000, "UDP match timed out");
        // ~250 Hz loop: faster than real time, but packets still flow through the kernel.
        next_tick += Duration::from_millis(4);
        if let Some(d) = next_tick.checked_duration_since(Instant::now()) {
            std::thread::sleep(d);
        }
    }
    for i in 0..2 {
        sessions[i].as_mut().unwrap().settle(&mut sims[i]);
    }
    assert_eq!(sims[0].checksum(), sims[1].checksum());
    assert_eq!(sims[0].checksum(), reference(2, config.input_delay, frames));
    assert!(sessions[0].as_ref().unwrap().stats().rtt_ms[1].is_some());
}
