use super::{DaemonConnection, request};
use anyhow::Context;
use futures::future::BoxFuture;
use tokio::{
    io::BufReader,
    net::{TcpStream, ToSocketAddrs},
};
use tracing::info;

pub struct TcpConnection {
    inner: BufReader<TcpStream>,
    in_flight: bool,
}

impl TcpConnection {
    pub async fn connect(addr: impl ToSocketAddrs) -> anyhow::Result<Box<Self>> {
        info!("connecting to remote TCP service");
        let inner = TcpStream::connect(addr).await?;
        Ok(Box::new(Self {
            inner: BufReader::new(inner),
            in_flight: false,
        }))
    }
}

impl DaemonConnection for TcpConnection {
    fn request<'a>(&'a mut self, payload: &'a str) -> BoxFuture<'a, anyhow::Result<String>> {
        Box::pin(async { request(&mut self.inner, payload, &mut self.in_flight).await })
    }

    fn new_connection(&self) -> BoxFuture<'_, anyhow::Result<Box<dyn DaemonConnection>>> {
        Box::pin(async {
            let peer_addr = self
                .inner
                .get_ref()
                .peer_addr()
                .context("Could not read peer address")?;

            Ok(Self::connect(peer_addr).await? as Box<dyn DaemonConnection>)
        })
    }
}
