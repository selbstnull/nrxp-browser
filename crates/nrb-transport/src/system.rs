use std::io;

use async_trait::async_trait;
use tokio::net::{TcpStream, UdpSocket};

use crate::{BoxDatagramFlow, BoxStream, Caps, DatagramFlow, Transport};

/// Обычные сокеты ОС. Только для тестов и отладки: DNS резолвится локально, трафик идёт мимо туннеля.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemTransport;

#[async_trait]
impl Transport for SystemTransport {
    async fn open_tcp(&self, host: &str, port: u16) -> io::Result<BoxStream> {
        let stream = TcpStream::connect((host, port)).await?;
        stream.set_nodelay(true)?;
        Ok(Box::new(stream))
    }

    async fn open_udp(&self, host: &str, port: u16) -> io::Result<BoxDatagramFlow> {
        let target = tokio::net::lookup_host((host, port))
            .await?
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "адрес не найден"))?;
        let bind = if target.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        };
        let socket = UdpSocket::bind(bind).await?;
        socket.connect(target).await?;
        Ok(Box::new(SystemDatagrams(socket)))
    }

    fn caps(&self) -> Caps {
        Caps::default()
    }

    fn name(&self) -> &'static str {
        "system"
    }
}

struct SystemDatagrams(UdpSocket);

#[async_trait]
impl DatagramFlow for SystemDatagrams {
    fn send(&self, payload: &[u8]) -> io::Result<()> {
        match self.0.try_send(payload) {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(()),
            Err(e) => Err(e),
        }
    }

    async fn recv(&self) -> io::Result<bytes::Bytes> {
        let mut buf = vec![0u8; 65_535];
        let n = self.0.recv(&mut buf).await?;
        buf.truncate(n);
        Ok(buf.into())
    }

    fn max_payload(&self) -> usize {
        1200
    }
}
