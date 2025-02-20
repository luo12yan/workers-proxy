use std::io::{Error, ErrorKind, Result};
use std::net::{Ipv4Addr, Ipv6Addr};

use crate::ext::StreamExt;
use crate::protocol;
use crate::websocket::WebSocketStream;
use base64::{engine::general_purpose, Engine as _};
use tokio::io::{copy_bidirectional, AsyncReadExt, AsyncWriteExt};
use worker::*;

pub fn parse_early_data(data: Option<String>) -> Result<Option<Vec<u8>>> {
    if let Some(data) = data {
        if !data.is_empty() {
            let s = data.replace('+', "-").replace('/', "_").replace("=", "");
            match general_purpose::URL_SAFE_NO_PAD.decode(s) {
                Ok(early_data) => return Ok(Some(early_data)),
                Err(err) => return Err(Error::new(ErrorKind::Other, err.to_string())),
            }
        }
    }
    Ok(None)
}

pub fn parse_user_id(user_id: &str) -> Vec<u8> {
    let mut hex_bytes = user_id
        .as_bytes()
        .iter()
        .filter_map(|b| match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        })
        .fuse();

    let mut bytes = Vec::new();
    while let (Some(h), Some(l)) = (hex_bytes.next(), hex_bytes.next()) {
        bytes.push((h << 4) | l)
    }
    bytes
}

pub async fn run_tunnel(
    mut client_socket: WebSocketStream<'_>,
    user_id: Vec<u8>,
    proxy_ip: Vec<String>,
) -> Result<()> {
    // read version
    if client_socket.read_u8().await? != protocol::VERSION {
        return Err(Error::new(ErrorKind::InvalidData, "invalid version"));
    }

    // verify user_id
    if client_socket.read_bytes(16).await? != user_id {
        return Err(Error::new(ErrorKind::InvalidData, "invalid user id"));
    }

    // ignore addons
    let length = client_socket.read_u8().await?;
    _ = client_socket.read_bytes(length as usize).await?;

    // read network type
    let network_type = client_socket.read_u8().await?;

    // read remote port
    let remote_port = client_socket.read_u16().await?;

    // read remote address
    let remote_addr = match client_socket.read_u8().await? {
        protocol::ADDRESS_TYPE_DOMAIN => {
            let length = client_socket.read_u8().await?;
            client_socket.read_string(length as usize).await?
        }
        protocol::ADDRESS_TYPE_IPV4 => {
            Ipv4Addr::from_bits(client_socket.read_u32().await?).to_string()
        }
        protocol::ADDRESS_TYPE_IPV6 => format!(
            "[{}]",
            Ipv6Addr::from_bits(client_socket.read_u128().await?)
        ),
        _ => {
            return Err(Error::new(ErrorKind::InvalidData, "invalid address type"));
        }
    };

    // process outbound
    match network_type {
        protocol::NETWORK_TYPE_TCP => {
            let addrs = [vec![remote_addr], proxy_ip].concat();
            // try to connect to remote
            for target in &addrs {
                match process_tcp_outbound(&mut client_socket, target, remote_port).await {
                    Ok(_) => {
                        // normal closed
                        return Ok(());
                    }
                    Err(e) => {
                        // connection reset
                        if e.kind() != ErrorKind::ConnectionReset {
                            return Err(Error::new(
                                e.kind(),
                                format!("Connection is reset error,{:?}", e),
                            ));
                        }

                        // continue to next target
                        continue;
                    }
                }
            }

            Err(Error::new(
                ErrorKind::InvalidData,
                format!("no target to connect,{:?}", addrs),
            ))
        }
        protocol::NETWORK_TYPE_UDP => {
            process_udp_outbound(&mut client_socket, &remote_addr, remote_port).await
        }
        unknown => Err(Error::new(
            ErrorKind::InvalidData,
            format!("unsupported network type: {:?}", unknown),
        )),
    }
}

async fn process_tcp_outbound(
    client_socket: &mut WebSocketStream<'_>,
    target: &str,
    port: u16,
) -> Result<()> {
    // connect to remote socket
    let mut remote_socket = Socket::builder().connect(target, port).map_err(|e| {
        Error::new(
            ErrorKind::ConnectionAborted,
            format!("connect to remote failed: {}", e),
        )
    })?;

    // check remote socket
    remote_socket.opened().await.map_err(|e| {
        Error::new(
            ErrorKind::ConnectionReset,
            format!("remote socket not opened: {}", e),
        )
    })?;

    // send response header
    client_socket
        .write(&protocol::RESPONSE)
        .await
        .map_err(|e| {
            Error::new(
                ErrorKind::ConnectionAborted,
                format!("send response header failed: {}", e),
            )
        })?;

    // forward data
    copy_bidirectional(client_socket, &mut remote_socket)
        .await
        .map_err(|e| {
            Error::new(
                ErrorKind::ConnectionAborted,
                format!("forward data between client and remote failed: {}", e),
            )
        })?;

    Ok(())
}

async fn process_udp_outbound(
    client_socket: &mut WebSocketStream<'_>,
    _: &str,
    port: u16,
) -> Result<()> {
    // check port (only support dns query)
    if port != 53 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "not supported udp proxy yet",
        ));
    }

    // send response header
    client_socket
        .write(&protocol::RESPONSE)
        .await
        .map_err(|e| {
            Error::new(
                ErrorKind::ConnectionAborted,
                format!("send response header failed: {}", e),
            )
        })?;

    // forward data
    loop {
        // read packet length
        let length = client_socket.read_u16().await;
        if length.is_err() {
            return Ok(());
        }

        // read dns packet
        let packet = client_socket.read_bytes(length.unwrap() as usize).await.map_err(|e| {
            Error::new(
                e.kind(),
                format!("read dns packet error: {}", e),
            )
        })?;

        // create request
        let request = Request::new_with_init("https://1.1.1.1/dns-query", &{
            // create request
            let mut init = RequestInit::new();
            init.method = Method::Post;
            init.headers = Headers::new();
            init.body = Some(packet.into());

            // set headers
            _ = init.headers.set("Content-Type", "application/dns-message");

            init
        })
        .unwrap();

        // invoke dns-over-http resolver
        let mut response = Fetch::Request(request).send().await.map_err(|e| {
            Error::new(
                ErrorKind::ConnectionAborted,
                format!("send DNS-over-HTTP request failed: {}", e),
            )
        })?;

        // read response
        let data = response.bytes().await.map_err(|e| {
            Error::new(
                ErrorKind::ConnectionAborted,
                format!("DNS-over-HTTP response body error: {}", e),
            )
        })?;

        // write response
        client_socket.write_u16(data.len() as u16).await?;
        client_socket.write_all(&data).await?;
    }
}
