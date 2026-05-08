use std::net::ToSocketAddrs;
use std::sync::Arc;
use std::time::Duration;

use backoff::ExponentialBackoffBuilder;
use log::{error, warn};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::Mutex;
use tokio::time::timeout;

use crate::error::{Error, Result};
use crate::protocol::{Command, Frame, IdentifyConfig, MAGIC_V2, Message, Protocol, ProtocolError};

#[derive(Debug)]
pub struct Connection {
    read_half: Mutex<OwnedReadHalf>,
    write_half: Mutex<OwnedWriteHalf>,
    addr: String,
    identify_config: IdentifyConfig,
    auth_secret: Option<String>,
    read_timeout: Duration,
    write_timeout: Duration,
}

impl Connection {
    pub async fn new<A: ToSocketAddrs + std::fmt::Display>(
        addr: A,
        identify_config: Option<IdentifyConfig>,
        auth_secret: Option<String>,
        read_timeout: Duration,
        write_timeout: Duration,
    ) -> Result<Self> {
        let addr_str = addr.to_string();
        let stream = Self::connect_with_retry(
            &addr_str,
            Duration::from_secs(5),
            read_timeout,
            write_timeout,
        )
        .await?;

        let (read_half, write_half) = stream.into_split();
        let conn = Self {
            read_half: Mutex::new(read_half),
            write_half: Mutex::new(write_half),
            addr: addr_str,
            identify_config: identify_config.unwrap_or_default(),
            auth_secret,
            read_timeout,
            write_timeout,
        };

        conn.initialize().await?;

        Ok(conn)
    }

    async fn initialize(&self) -> Result<()> {
        let mut w = self.write_half.lock().await;
        let mut r = self.read_half.lock().await;

        w.write_all(MAGIC_V2).await?;
        let identify_bytes = Command::Identify(self.identify_config.clone()).to_bytes()?;
        w.write_all(&identify_bytes).await?;
        w.flush().await?;

        let mut buf = [0u8; 4];
        r.read_exact(&mut buf).await?;
        let size = u32::from_be_bytes(buf);
        if size == 0 {
            return Err(Error::Protocol(ProtocolError::InvalidFrameSize));
        }
        r.read_exact(&mut buf).await?;
        if u32::from_be_bytes(buf) != 0 {
            return Err(Error::Protocol(ProtocolError::InvalidFrameType(
                i32::from_be_bytes(buf),
            )));
        }
        let mut resp = vec![0u8; (size - 4) as usize];
        r.read_exact(&mut resp).await?;

        if let Some(secret) = &self.auth_secret {
            let auth_bytes = Command::Auth(Some(secret.clone())).to_bytes()?;
            w.write_all(&auth_bytes).await?;
            w.flush().await?;
            r.read_exact(&mut buf).await?;
            let size = u32::from_be_bytes(buf);
            if size == 0 {
                return Err(Error::Auth("认证响应大小为0".to_string()));
            }
            r.read_exact(&mut buf).await?;
            if u32::from_be_bytes(buf) != 0 {
                return Err(Error::Auth(format!(
                    "认证失败，帧类型 {}",
                    u32::from_be_bytes(buf)
                )));
            }
            let mut resp = vec![0u8; (size - 4) as usize];
            r.read_exact(&mut resp).await?;
        }

        Ok(())
    }

    pub async fn reconnect(&self) -> Result<()> {
        let stream = Self::connect_with_retry(
            &self.addr,
            Duration::from_secs(5),
            self.read_timeout,
            self.write_timeout,
        )
        .await?;

        let (new_read, new_write) = stream.into_split();
        *self.read_half.lock().await = new_read;
        *self.write_half.lock().await = new_write;

        self.initialize().await
    }

    pub async fn connect_with_retry(
        addr: &str,
        timeout_duration: Duration,
        _read_timeout: Duration,
        _write_timeout: Duration,
    ) -> Result<TcpStream> {
        let backoff = ExponentialBackoffBuilder::new()
            .with_initial_interval(Duration::from_millis(100))
            .with_max_interval(Duration::from_secs(1))
            .with_multiplier(2.0)
            .with_max_elapsed_time(Some(timeout_duration))
            .build();

        let addr_clone = addr.to_string();
        let result = backoff::future::retry_notify(
            backoff,
            || async {
                match TcpStream::connect(&addr_clone).await {
                    Ok(stream) => Ok(stream),
                    Err(e) => Err(backoff::Error::transient(Error::Io(e))),
                }
            },
            |err, duration| {
                warn!(
                    "连接到 {} 失败: {:?}, 将在 {:?} 后重试",
                    addr_clone, err, duration
                );
            },
        )
        .await;

        result.map_err(|e| Error::Connection(format!("无法连接到 {}: {:?}", addr, e)))
    }

    pub async fn send_command(&self, command: Command) -> Result<()> {
        let bytes = command.to_bytes()?;
        let mut w = self.write_half.lock().await;
        timeout(self.write_timeout, async {
            w.write_all(&bytes).await?;
            w.flush().await
        })
        .await??;
        Ok(())
    }

    pub async fn read_frame(&self) -> Result<Frame> {
        let mut r = self.read_half.lock().await;

        let mut size_buf = [0u8; 4];
        timeout(self.read_timeout, r.read_exact(&mut size_buf)).await??;
        let size = u32::from_be_bytes(size_buf);

        if size < 4 {
            return Err(Error::Protocol(ProtocolError::InvalidFrameSize));
        }

        let mut frame_type_buf = [0u8; 4];
        timeout(self.read_timeout, r.read_exact(&mut frame_type_buf)).await??;
        let frame_type = i32::from_be_bytes(frame_type_buf);

        match frame_type {
            0..=2 => {
                let data_size = size - 4;
                let mut data = vec![0u8; data_size as usize];
                timeout(self.read_timeout, r.read_exact(&mut data)).await??;

                let mut frame_data = Vec::with_capacity(size as usize);
                frame_data.extend_from_slice(&frame_type_buf);
                frame_data.extend_from_slice(&data);

                Protocol::decode_frame(&frame_data)
            }
            _ => Err(Error::Protocol(ProtocolError::InvalidFrameType(frame_type))),
        }
    }

    pub async fn handle_heartbeat(&self) -> Result<()> {
        self.send_command(Command::Nop).await
    }

    pub async fn ping(&self, timeout_duration: Option<Duration>) -> Result<()> {
        let timeout_dur = timeout_duration.unwrap_or(Duration::from_secs(5));
        timeout(timeout_dur, self.send_command(Command::Nop))
            .await
            .map_err(|_| Error::Timeout(format!("Ping 操作超时 ({}秒)", timeout_dur.as_secs())))?
    }

    pub async fn read_message(&self) -> Result<Option<Message>> {
        match self.read_frame().await {
            Ok(Frame::Message(msg)) => Ok(Some(msg)),
            Ok(Frame::Response(_)) => Ok(None),
            Ok(Frame::Error(data)) => {
                error!("NSQ错误响应: {:?}", String::from_utf8_lossy(&data));
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    pub fn addr(&self) -> &str {
        &self.addr
    }

    pub async fn close(&self) -> Result<()> {
        self.write_half
            .lock()
            .await
            .shutdown()
            .await
            .map_err(Error::from)
    }
}

pub async fn close_connection(connection: &Arc<Connection>) -> Result<()> {
    connection.send_command(Command::Cls).await
}
