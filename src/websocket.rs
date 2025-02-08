use futures_util::Stream;
use std::{
    io::{Error, ErrorKind, Result},
    pin::Pin,
    task::{Context, Poll},
};

use bytes::{BufMut, BytesMut};
use pin_project::pin_project;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use worker::{
    console_error, EventStream, Request, Response, WebSocket, WebSocketPair, WebsocketEvent,
};

use crate::proxy::{parse_early_data, run_tunnel};

pub fn ws_handler(
    user_id: Vec<u8>,
    proxy_ip: Vec<String>,
    ws_protocol: Option<Vec<u8>>,
) -> worker::Result<Response> {
    let ws = WebSocketPair::new()?;
    let client = ws.client;
    let server = ws.server;

    server.accept()?;

    wasm_bindgen_futures::spawn_local(async move {
        // create websocket stream
        let socket = WebSocketStream::new(
            &server,
            server.events().expect("could not open stream"),
            ws_protocol,
        );

        // into tunnel
        if let Err(err) = run_tunnel(socket, user_id, proxy_ip).await {
            // log error
            console_error!("error: {}", err);

            // close websocket connection
            _ = server.close(Some(1003), Some("invalid request"));
        }
    });

    Response::from_websocket(client)
}

#[pin_project]
pub struct WebSocketStream<'a> {
    ws: &'a WebSocket,
    #[pin]
    stream: EventStream<'a>,
    buffer: BytesMut,
}

impl<'a> WebSocketStream<'a> {
    pub fn new(ws: &'a WebSocket, stream: EventStream<'a>, early_data: Option<Vec<u8>>) -> Self {
        let mut buffer = BytesMut::new();
        if let Some(data) = early_data {
            buffer.put_slice(&data)
        }

        Self { ws, stream, buffer }
    }
}

impl AsyncRead for WebSocketStream<'_> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<Result<()>> {
        let mut this = self.project();

        loop {
            let amt = std::cmp::min(this.buffer.len(), buf.remaining());
            if amt > 0 {
                buf.put_slice(&this.buffer.split_to(amt));
                return Poll::Ready(Ok(()));
            }

            match this.stream.as_mut().poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(Ok(WebsocketEvent::Message(msg)))) => {
                    if let Some(data) = msg.bytes() {
                        this.buffer.put_slice(&data);
                    };
                    continue;
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Err(Error::new(ErrorKind::Other, e.to_string())))
                }
                _ => return Poll::Ready(Ok(())), // None or Close event, return Ok to indicate stream end
            }
        }
    }
}

impl AsyncWrite for WebSocketStream<'_> {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, buf: &[u8]) -> Poll<Result<usize>> {
        if let Err(e) = self.ws.send_with_bytes(buf) {
            return Poll::Ready(Err(Error::new(ErrorKind::Other, e.to_string())));
        }

        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<()>> {
        if let Err(e) = self.ws.close(None, Some("normal close")) {
            return Poll::Ready(Err(Error::new(ErrorKind::Other, e.to_string())));
        }

        Poll::Ready(Ok(()))
    }
}
