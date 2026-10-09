//! Transport addresses that can be sent over the wire (the host tells peers about each other).

use std::fmt::Debug;
use std::hash::Hash;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use crate::wire::{Reader, Writer};

/// An endpoint address usable by a [`crate::Transport`].
pub trait NetAddr: Copy + Eq + Hash + Debug + 'static {
    fn encode(&self, w: &mut Writer);
    fn decode(r: &mut Reader<'_>) -> Option<Self>;
}

impl NetAddr for SocketAddr {
    fn encode(&self, w: &mut Writer) {
        match self.ip() {
            IpAddr::V4(ip) => {
                w.u8(4);
                w.bytes(&ip.octets());
            }
            IpAddr::V6(ip) => {
                w.u8(6);
                w.bytes(&ip.octets());
            }
        }
        w.u16(self.port());
    }

    fn decode(r: &mut Reader<'_>) -> Option<Self> {
        let ip = match r.u8()? {
            4 => {
                let b: [u8; 4] = r.bytes(4)?.try_into().ok()?;
                IpAddr::V4(Ipv4Addr::from(b))
            }
            6 => {
                let b: [u8; 16] = r.bytes(16)?.try_into().ok()?;
                IpAddr::V6(Ipv6Addr::from(b))
            }
            _ => return None,
        };
        Some(SocketAddr::new(ip, r.u16()?))
    }
}
