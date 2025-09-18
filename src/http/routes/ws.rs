use picoserve::response::ws;
use picoserve::io::embedded_io_async; // for Read/Write trait bounds

pub(crate) async fn ws_handler(
    upgrade: ws::WebSocketUpgrade,
) -> impl picoserve::response::IntoResponse {
    upgrade.on_upgrade(HelloWs)
}

struct HelloWs;

impl ws::WebSocketCallback for HelloWs {
    async fn run<R: embedded_io_async::Read, W: embedded_io_async::Write<Error = R::Error>>(
        self,
        mut rx: ws::SocketRx<R>,
        mut tx: ws::SocketTx<W>,
    ) -> Result<(), W::Error> {
        // greet once
        tx.send_text("hello").await?;

        let mut buf = [0u8; 1024];
        loop {
            match rx.next_message(&mut buf).await {
                Ok(ws::Message::Text(s))   => tx.send_text(s).await?,
                Ok(ws::Message::Binary(b)) => tx.send_binary(b).await?,
                Ok(ws::Message::Ping(p))   => tx.send_pong(p).await?,
                Ok(ws::Message::Pong(_))   => { /* ignore */ }
                Ok(ws::Message::Close(_))  => break,
                Err(_)                     => break,
            }
        }

        tx.close(None).await
    }
}
