pub mod tcp;
pub mod unix;

use anyhow::{anyhow, ensure};
use futures::future::BoxFuture;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

pub trait DaemonConnection {
    fn request<'a>(&'a mut self, payload: &'a str) -> BoxFuture<'a, anyhow::Result<String>>;

    /// Establish a new connection to the same service
    fn new_connection(&self) -> BoxFuture<'_, anyhow::Result<Box<dyn DaemonConnection>>>;
}

async fn request(
    socket: &mut BufReader<impl AsyncRead + AsyncWrite + Unpin>,
    payload: &str,
    in_flight: &mut bool,
) -> anyhow::Result<String> {
    // Dropping a request can leave an unread or partial response in the socket.
    // Keep this flag set across cancellation so the client reconnects before reuse.
    ensure!(
        !*in_flight,
        "Previous request was interrupted; reconnect required"
    );
    *in_flight = true;
    if !socket.buffer().is_empty() {
        return Err(anyhow!("Another request was not processed properly"));
    }

    socket.write_all(payload.as_bytes()).await?;
    socket.write_all(b"\n").await?;

    let mut response_payload = String::new();
    socket.read_line(&mut response_payload).await?;
    // Leave an incomplete exchange marked without turning a parse failure into
    // an automatic retry of a possibly completed write request.
    if response_payload.ends_with('\n') {
        *in_flight = false;
    }

    Ok(response_payload)
}
