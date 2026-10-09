//! In-memory network with a virtual clock and configurable latency, jitter and loss.
//!
//! Deterministic for a given seed: tests are reproducible.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::sync::{Arc, Mutex};

use super::Transport;
use crate::addr::NetAddr;
use crate::wire::{Reader, Writer};

/// Address of a loopback endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LoopbackAddr(pub u32);

impl NetAddr for LoopbackAddr {
    fn encode(&self, w: &mut Writer) {
        w.u32(self.0);
    }
    fn decode(r: &mut Reader<'_>) -> Option<Self> {
        Some(LoopbackAddr(r.u32()?))
    }
}

/// Link characteristics (one-way).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoopbackConfig {
    /// Base one-way latency.
    pub latency_ms: u64,
    /// Uniform extra delay in `0..=jitter_ms` per packet (packets may reorder).
    pub jitter_ms: u64,
    /// Probability (0..1) that a packet is dropped.
    pub loss: f64,
    /// Probability (0..1) that a packet is delivered twice.
    pub duplicate: f64,
    pub seed: u64,
}

impl Default for LoopbackConfig {
    fn default() -> Self {
        LoopbackConfig { latency_ms: 0, jitter_ms: 0, loss: 0.0, duplicate: 0.0, seed: 1 }
    }
}

struct Queued {
    deliver_at: u64,
    order: u64,
    from: LoopbackAddr,
    data: Vec<u8>,
}

impl PartialEq for Queued {
    fn eq(&self, o: &Self) -> bool {
        (self.deliver_at, self.order) == (o.deliver_at, o.order)
    }
}
impl Eq for Queued {}
impl PartialOrd for Queued {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Queued {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        (self.deliver_at, self.order).cmp(&(o.deliver_at, o.order))
    }
}

struct Hub {
    config: LoopbackConfig,
    /// Per-destination overrides of the link config.
    overrides: HashMap<LoopbackAddr, LoopbackConfig>,
    now_ms: u64,
    rng: u64,
    order: u64,
    next_addr: u32,
    queues: HashMap<LoopbackAddr, BinaryHeap<Reverse<Queued>>>,
    sent: u64,
    dropped: u64,
}

impl Hub {
    fn rand(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn rand_f64(&mut self) -> f64 {
        (self.rand() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// A shared in-memory network. Clone it freely; all clones share the same state.
#[derive(Clone)]
pub struct LoopbackNetwork {
    hub: Arc<Mutex<Hub>>,
}

impl LoopbackNetwork {
    pub fn new(config: LoopbackConfig) -> Self {
        LoopbackNetwork {
            hub: Arc::new(Mutex::new(Hub {
                rng: config.seed | 1,
                config,
                overrides: HashMap::new(),
                now_ms: 0,
                order: 0,
                next_addr: 1,
                queues: HashMap::new(),
                sent: 0,
                dropped: 0,
            })),
        }
    }

    /// Create a new endpoint with a fresh address.
    pub fn endpoint(&self) -> LoopbackTransport {
        let mut h = self.hub.lock().unwrap();
        let addr = LoopbackAddr(h.next_addr);
        h.next_addr += 1;
        h.queues.insert(addr, BinaryHeap::new());
        LoopbackTransport { hub: self.hub.clone(), addr }
    }

    /// Advance the virtual clock (packets become deliverable at `deliver_at <= now`).
    pub fn set_time(&self, now_ms: u64) {
        self.hub.lock().unwrap().now_ms = now_ms;
    }

    pub fn now(&self) -> u64 {
        self.hub.lock().unwrap().now_ms
    }

    /// Use a different link config for packets *to* `addr`.
    pub fn set_link(&self, addr: LoopbackAddr, config: LoopbackConfig) {
        self.hub.lock().unwrap().overrides.insert(addr, config);
    }

    /// `(packets sent, packets dropped)`.
    pub fn counters(&self) -> (u64, u64) {
        let h = self.hub.lock().unwrap();
        (h.sent, h.dropped)
    }
}

/// One endpoint of a [`LoopbackNetwork`].
pub struct LoopbackTransport {
    hub: Arc<Mutex<Hub>>,
    addr: LoopbackAddr,
}

impl LoopbackTransport {
    pub fn addr(&self) -> LoopbackAddr {
        self.addr
    }
}

impl Transport for LoopbackTransport {
    type Addr = LoopbackAddr;

    fn send_to(&mut self, data: &[u8], to: LoopbackAddr) {
        let mut h = self.hub.lock().unwrap();
        h.sent += 1;
        let cfg = h.overrides.get(&to).copied().unwrap_or(h.config);
        if !h.queues.contains_key(&to) || h.rand_f64() < cfg.loss {
            h.dropped += 1;
            return;
        }
        let copies = if h.rand_f64() < cfg.duplicate { 2 } else { 1 };
        for _ in 0..copies {
            let jitter = if cfg.jitter_ms > 0 { h.rand() % (cfg.jitter_ms + 1) } else { 0 };
            let deliver_at = h.now_ms + cfg.latency_ms + jitter;
            h.order += 1;
            let q = Queued { deliver_at, order: h.order, from: self.addr, data: data.to_vec() };
            h.queues.get_mut(&to).unwrap().push(Reverse(q));
        }
    }

    fn recv_from(&mut self) -> Option<(Vec<u8>, LoopbackAddr)> {
        let mut h = self.hub.lock().unwrap();
        let now = h.now_ms;
        let q = h.queues.get_mut(&self.addr)?;
        if q.peek()?.0.deliver_at <= now {
            let Reverse(p) = q.pop()?;
            Some((p.data, p.from))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latency_and_loss() {
        let net = LoopbackNetwork::new(LoopbackConfig { latency_ms: 50, ..Default::default() });
        let mut a = net.endpoint();
        let mut b = net.endpoint();
        a.send_to(b"hi", b.addr());
        assert!(b.recv_from().is_none());
        net.set_time(49);
        assert!(b.recv_from().is_none());
        net.set_time(50);
        assert_eq!(b.recv_from(), Some((b"hi".to_vec(), a.addr())));

        let lossy = LoopbackNetwork::new(LoopbackConfig { loss: 0.5, seed: 7, ..Default::default() });
        let mut a = lossy.endpoint();
        let mut b = lossy.endpoint();
        for _ in 0..1000 {
            a.send_to(b"x", b.addr());
        }
        let mut n = 0;
        while b.recv_from().is_some() {
            n += 1;
        }
        assert!((400..600).contains(&n), "{n}");
    }
}
