//! Unreliable datagram transports.

pub mod loopback;
pub mod udp;

use crate::addr::NetAddr;

/// An unreliable, unordered, non-blocking datagram socket.
///
/// Sessions own their transport; they never block on it.
pub trait Transport {
    type Addr: NetAddr;
    /// Fire-and-forget send. Errors are swallowed (the protocol is loss tolerant).
    fn send_to(&mut self, data: &[u8], to: Self::Addr);
    /// Next received datagram, or `None` when nothing is pending.
    fn recv_from(&mut self) -> Option<(Vec<u8>, Self::Addr)>;
}
