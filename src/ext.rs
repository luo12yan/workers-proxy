use std::io::Result;
use tokio::io::AsyncReadExt;
use worker::Request;
#[allow(dead_code)]
pub trait StreamExt {
    async fn read_string(&mut self, n: usize) -> Result<String>;
    async fn read_bytes(&mut self, n: usize) -> Result<Vec<u8>>;
}

impl<T: AsyncReadExt + Unpin + ?Sized> StreamExt for T {
    async fn read_string(&mut self, n: usize) -> Result<String> {
        self.read_bytes(n).await.map(|bytes| {
            String::from_utf8(bytes).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("invalid string: {}", e),
                )
            })
        })?
    }

    async fn read_bytes(&mut self, n: usize) -> Result<Vec<u8>> {
        let mut buffer = vec![0u8; n];
        self.read_exact(&mut buffer).await?;

        Ok(buffer)
    }
}

pub trait RequestExt {
    fn header(&self, name: &str) -> Option<String>;
}

impl RequestExt for Request {
    fn header(&self, name: &str) -> Option<String> {
        self.headers().get(name).map_or(None, |s| s)
    }
}
