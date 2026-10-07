use std::io;

use async_trait::async_trait;
use tokio::net::TcpStream;

use crate::{BoxStream, Caps, Transport};

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

    fn caps(&self) -> Caps {
        Caps::default()
    }

    fn name(&self) -> &'static str {
        "system"
    }
}
