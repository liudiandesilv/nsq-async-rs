use std::net::ToSocketAddrs;
use std::sync::Arc;
use std::time::Duration;

use backoff::ExponentialBackoffBuilder;
use log::{error, info, warn};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::time::timeout;

use crate::error::{Error, Result};
use crate::protocol::{Command, Frame, IdentifyConfig, Message, Protocol, ProtocolError, MAGIC_V2};

/// TCP连接管理器
#[derive(Debug)]
pub struct Connection {
    /// TCP连接
    stream: Mutex<TcpStream>,
    /// NSQ服务器地址
    addr: String,
    /// 身份配置
    identify_config: IdentifyConfig,
    /// 是否已认证
    auth_secret: Option<String>,
    read_timeout: Duration,
    write_timeout: Duration,
}

impl Connection {
    /// 重新建立连接
    pub async fn reconnect(&self) -> Result<()> {
        let stream = Self::connect_with_retry(
            &self.addr,
            Duration::from_secs(5),
            self.read_timeout,
            self.write_timeout,
        ).await?;
        
        // 替换现有的流
        let mut current_stream = self.stream.lock().await;
        *current_stream = stream;
        
        // 重新初始化连接
        drop(current_stream); // 释放锁，避免死锁
        self.initialize().await?;
        
        Ok(())
    }
    
    /// 创建新的连接
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

        let connection = Self {
            stream: Mutex::new(stream),
            addr: addr_str,
            identify_config: identify_config.unwrap_or_default(),
            auth_secret,
            read_timeout,
            write_timeout,
        };

        // 初始化连接
        connection.initialize().await?;

        Ok(connection)
    }

    /// 使用重试机制连接到NSQ服务器
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

        match result {
            Ok(stream) => {
                // info!("成功连接到 NSQ 服务器: {}", addr);
                Ok(stream)
            }
            Err(e) => Err(Error::Connection(format!("无法连接到 {}: {:?}", addr, e))),
        }
    }

    /// 初始化连接
    async fn initialize(&self) -> Result<()> {
        let mut stream = self.stream.lock().await;

        // 发送魔术字
        stream.write_all(MAGIC_V2).await?;

        // 发送识别信息
        let identify_cmd = Command::Identify(self.identify_config.clone());
        let identify_bytes = identify_cmd.to_bytes()?;
        stream.write_all(&identify_bytes).await?;
        stream.flush().await?;

        // 读取和处理响应
        let mut buf = [0u8; 4];
        stream.read_exact(&mut buf).await?;
        let size = u32::from_be_bytes(buf);

        if size == 0 {
            return Err(Error::Protocol(ProtocolError::InvalidFrameSize));
        }

        // 读取帧类型
        stream.read_exact(&mut buf).await?;
        let frame_type = u32::from_be_bytes(buf);

        if frame_type != 0 {
            return Err(Error::Protocol(ProtocolError::InvalidFrameType(
                frame_type as i32,
            )));
        }

        // 读取响应体
        let mut response_data = vec![0u8; (size - 4) as usize];
        stream.read_exact(&mut response_data).await?;

        // 如果需要认证，发送认证命令
        if let Some(ref secret) = self.auth_secret {
            let auth_cmd = Command::Auth(Some(secret.clone()));
            let auth_bytes = auth_cmd.to_bytes()?;
            stream.write_all(&auth_bytes).await?;
            stream.flush().await?;

            // 读取认证响应 (简化版)
            let mut buf = [0u8; 4];
            stream.read_exact(&mut buf).await?;
            let size = u32::from_be_bytes(buf);

            if size == 0 {
                return Err(Error::Auth("认证响应大小为0".to_string()));
            }

            // 读取帧类型
            stream.read_exact(&mut buf).await?;
            let frame_type = u32::from_be_bytes(buf);

            if frame_type != 0 {
                return Err(Error::Auth(format!("认证失败，帧类型 {}", frame_type)));
            }

            // 读取响应体
            let mut response_data = vec![0u8; (size - 4) as usize];
            stream.read_exact(&mut response_data).await?;
        }

        Ok(())
    }

    /// 发送命令到NSQ服务器
    pub async fn send_command(&self, command: Command) -> Result<()> {
        let mut stream = self.stream.lock().await;
        let bytes = command.to_bytes()?;
        stream.write_all(&bytes).await?;
        stream.flush().await?;
        Ok(())
    }

    /// 读取下一个NSQ帧
    pub async fn read_frame(&self) -> Result<Frame> {
        let mut stream_guard = self.stream.lock().await;

        // 读取帧大小 (4字节)
        let mut size_buf = [0u8; 4];
        timeout(self.read_timeout, stream_guard.read_exact(&mut size_buf)).await??;
        let size = u32::from_be_bytes(size_buf);

        if size < 4 {
            return Err(Error::Protocol(ProtocolError::InvalidFrameSize));
        }

        // 读取帧类型 (4字节)
        let mut frame_type_buf = [0u8; 4];
        timeout(
            self.read_timeout,
            stream_guard.read_exact(&mut frame_type_buf),
        )
        .await??;
        let frame_type = i32::from_be_bytes(frame_type_buf);

        // 根据 NSQ 协议，帧类型应该是以下之一：
        // FrameTypeResponse = 0
        // FrameTypeError = 1
        // FrameTypeMessage = 2
        match frame_type {
            0..=2 => {
                // 读取帧数据
                let data_size = size - 4; // 减去帧类型的4字节
                let mut data = vec![0u8; data_size as usize];
                timeout(self.read_timeout, stream_guard.read_exact(&mut data)).await??;

                // 构造完整的帧数据（包括帧类型）
                let mut frame_data = Vec::with_capacity(size as usize);
                frame_data.extend_from_slice(&frame_type_buf);
                frame_data.extend_from_slice(&data);

                Protocol::decode_frame(&frame_data).map_err(Error::from)
            }
            _ => Err(Error::Protocol(ProtocolError::InvalidFrameType(frame_type))),
        }
    }

    /// 处理心跳帧
    pub async fn handle_heartbeat(&self) -> Result<()> {
        self.send_command(Command::Nop).await
    }

    /// 读取消息 - 参考Go客户端中的readLoop实现
    pub async fn read_message(&self) -> Result<Option<Message>> {
        match self.read_frame().await {
            Ok(Frame::Message(msg)) => {
                info!(
                    "收到消息 [ID: {:?}, 尝试次数: {}, 时间戳: {}]",
                    &msg.id, msg.attempts, msg.timestamp
                );
                Ok(Some(msg))
            }
            Ok(Frame::Response(_)) => Ok(None),
            Ok(Frame::Error(data)) => {
                error!("NSQ错误响应: {:?}", String::from_utf8_lossy(&data));
                Ok(None)
            }
            Err(e) => {
                error!("读取消息错误: {:?}", e);
                Err(e)
            }
        }
    }

    /// 获取连接的地址
    pub fn addr(&self) -> &str {
        &self.addr
    }

    pub async fn write_all(&self, buf: &[u8]) -> Result<()> {
        let mut stream = self.stream.lock().await;
        timeout(self.write_timeout, stream.write_all(buf)).await??;
        Ok(())
    }

    pub async fn read_exact(&self, buf: &mut [u8]) -> Result<()> {
        let mut stream = self.stream.lock().await;
        timeout(self.read_timeout, stream.read_exact(buf)).await??;
        Ok(())
    }

    pub async fn write_command(
        &self,
        name: &str,
        body: Option<&[u8]>,
        params: &[&str],
    ) -> Result<()> {
        let cmd = Protocol::encode_command(name, body, params);
        self.write_all(&cmd).await
    }

    pub async fn close(&self) -> Result<()> {
        self.stream
            .lock()
            .await
            .shutdown()
            .await
            .map_err(Error::from)
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // 在析构函数中发送CLS命令是不可能的，因为它需要异步上下文
        // 实际应用中应确保在丢弃连接前调用显式的关闭方法
    }
}

/// 在异步上下文中安全关闭连接
pub async fn close_connection(connection: &Arc<Connection>) -> Result<()> {
    connection.send_command(Command::Cls).await
}
