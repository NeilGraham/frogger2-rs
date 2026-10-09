//! Non-blocking UDP transport on `std::net`.

use std::io::ErrorKind;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};

use super::Transport;

/// A non-blocking UDP socket.
pub struct UdpTransport {
    socket: UdpSocket,
    buf: Vec<u8>,
    /// Send errors (ignored by the protocol, counted for diagnostics).
    pub send_errors: u64,
}

impl UdpTransport {
    /// Bind to `addr` (e.g. `"0.0.0.0:7777"`, or port 0 for an ephemeral port).
    pub fn bind<A: ToSocketAddrs>(addr: A) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(addr)?;
        socket.set_nonblocking(true)?;
        Ok(UdpTransport { socket, buf: vec![0; 2048], send_errors: 0 })
    }

    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    pub fn socket(&self) -> &UdpSocket {
        &self.socket
    }
}

impl Transport for UdpTransport {
    type Addr = SocketAddr;

    fn send_to(&mut self, data: &[u8], to: SocketAddr) {
        if self.socket.send_to(data, to).is_err() {
            self.send_errors += 1;
        }
    }

    fn recv_from(&mut self) -> Option<(Vec<u8>, SocketAddr)> {
        loop {
            match self.socket.recv_from(&mut self.buf) {
                Ok((n, from)) => return Some((self.buf[..n].to_vec(), from)),
                Err(e) if e.kind() == ErrorKind::WouldBlock => return None,
                // Windows reports ICMP port-unreachable as ConnectionReset on the next recv;
                // other transient errors are equally harmless for an unreliable protocol.
                Err(e) if e.kind() == ErrorKind::ConnectionReset => continue,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(_) => return None,
            }
        }
    }
}
