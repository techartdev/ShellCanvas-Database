mod socket;
pub use socket::set_tcp_proxy;
pub mod tls;

pub use socket::{
    connect_tcp, connect_uds, BufferedSocket, Socket, SocketIntoBox, WithSocket, WriteBuffer,
};
