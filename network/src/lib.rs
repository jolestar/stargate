#![feature(async_await)]

#[cfg(test)]
pub mod message;
pub mod peer;
pub mod error;
pub mod mem_stream;
pub mod p2p;
pub mod net;

mod tests {
    use crate::p2p::{new_network, NetConfig};
    use crate::mem_stream::{MemTcpStream, MemNetwork, MemListener};
    use std::net::SocketAddr;
    use futures::{Stream, Future, future};

    #[test]
    #[should_panic(expected = "not yet implemente")]
    fn test_new_network() {
        let cfg = NetConfig {
            addr: "".to_string(),
            max_sockets: 0,
            memory_stream: false,
            seeds: vec![]
        };
        let network = new_network::<
            MemTcpStream,
            future::Ready<MemTcpStream>,
            MemListener,
            MemNetwork,
        >(cfg);
    }
}