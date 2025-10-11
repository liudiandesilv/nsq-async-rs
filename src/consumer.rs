use async_trait::async_trait;
use log::{error, info};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::time::Duration;
use thiserror::Error;
use tokio::sync::Mutex;
use tokio::sync::mpsc;

use crate::connection::Connection;
use crate::error::{Error, Result};
use crate::protocol::{Command, Frame, Message as ProtocolMessage, ProtocolError};

#[derive(Debug, Error)]
pub enum ConsumerError {
    #[error("Invalid topic name: {0}")]
    InvalidTopic(String),
    #[error("Invalid channel name: {0}")]
    InvalidChannel(String),
    #[error("Connection error: {0}")]
    ConnectionError(String),
    #[error("Protocol error: {0}")]
    ProtocolError(String),
}

#[derive(Debug, Clone)]
pub struct Message {
    pub id: Vec<u8>,
    pub body: Vec<u8>,
    pub attempts: u16,
    pub timestamp: u64,
}

/// 消息处理器 trait
///
/// 实现此 trait 来处理从 NSQ 接收的消息。
///
/// # 自动响应模式（默认）
///
/// 当 `ConsumerConfig::disable_auto_response` 为 `false` 时（默认）：
/// - 如果 `handle_message` 返回 `Ok(())`，消息会自动发送 FIN 命令
/// - 如果 `handle_message` 返回 `Err(_)`，消息会自动发送 REQ 命令
///
/// # 手动响应模式
///
/// 当 `ConsumerConfig::disable_auto_response` 为 `true` 时：
/// - 消息不会自动发送 FIN/REQ
/// - 需要在 handler 中手动调用：
///   - `message.finish()` 完成消息处理
///   - `message.requeue(delay)` 重新入队消息
///   - `message.touch()` 重置消息超时
///
/// 或者，在自动响应模式下，也可以在单个消息上调用 `message.disable_auto_response()` 来禁用该消息的自动响应。
///
/// # 示例
///
/// ## 自动响应模式
///
/// ```rust,ignore
/// struct MyHandler;
///
/// #[async_trait]
/// impl Handler for MyHandler {
///     async fn handle_message(&self, message: Message) -> Result<()> {
///         // 处理消息
///         println!("收到消息: {:?}", String::from_utf8_lossy(&message.body));
///         
///         // 返回 Ok 会自动发送 FIN
///         Ok(())
///     }
/// }
/// ```
///
/// ## 手动响应模式
///
/// ```rust,ignore
/// struct MyHandler;
///
/// #[async_trait]
/// impl Handler for MyHandler {
///     async fn handle_message(&self, message: Message) -> Result<()> {
///         // 处理消息
///         match process(&message.body) {
///             Ok(_) => {
///                 // 手动发送 FIN
///                 message.finish().await?;
///             }
///             Err(_) => {
///                 // 手动重新入队，延迟 5 秒
///                 message.requeue(5000).await?;
///             }
///         }
///         
///         Ok(())
///     }
/// }
/// ```
#[async_trait]
pub trait Handler: Send + Sync + 'static {
    async fn handle_message(&self, message: ProtocolMessage) -> Result<()>;
}

pub struct ConsumerStats {
    pub messages_received: u64,
    pub messages_finished: u64,
    pub messages_requeued: u64,
    pub connections: i32,
}

#[derive(Debug, Clone)]
pub struct ConsumerConfig {
    pub max_in_flight: i32,
    pub max_attempts: u16,
    pub dial_timeout: Duration,
    pub read_timeout: Duration,
    pub write_timeout: Duration,
    pub lookup_poll_interval: Duration,
    pub lookup_poll_jitter: f64,
    pub max_requeue_delay: Duration,
    pub default_requeue_delay: Duration,
    pub shutdown_timeout: Duration,
    /// 是否使用指数退避策略进行重连
    pub backoff_strategy: bool,
    /// 是否禁用自动响应
    ///
    /// 当设置为 true 时，消息不会根据 Handler 的返回值自动发送 FIN/REQ
    /// 需要在 Handler 中手动调用 message.finish() 或 message.requeue()
    ///
    /// 这对于以下场景很有用：
    /// - 并发处理消息时需要异步确认
    /// - 批量处理消息
    /// - 需要精确控制消息确认时机
    pub disable_auto_response: bool,
}

impl Default for ConsumerConfig {
    fn default() -> Self {
        ConsumerConfig {
            max_in_flight: 1,
            max_attempts: 5,
            dial_timeout: Duration::from_secs(1),
            read_timeout: Duration::from_secs(60),
            write_timeout: Duration::from_secs(1),
            lookup_poll_interval: Duration::from_secs(60),
            lookup_poll_jitter: 0.3,
            max_requeue_delay: Duration::from_secs(15 * 60),
            default_requeue_delay: Duration::from_secs(90),
            shutdown_timeout: Duration::from_secs(30),
            backoff_strategy: true,
            disable_auto_response: false,
        }
    }
}

pub struct Consumer {
    topic: String,
    channel: String,
    config: ConsumerConfig,
    handler: Arc<dyn Handler + Send + Sync + 'static>,

    // Stats
    messages_received: AtomicU64,
    messages_finished: AtomicU64,
    messages_requeued: AtomicU64,

    // Connection management
    connections: Arc<Mutex<HashMap<String, Arc<Connection>>>>,
    total_rdy_count: AtomicI32,
    max_in_flight: AtomicI32,

    // Control
    is_running: AtomicBool,
    stop_chan: mpsc::Sender<()>,
}

struct ConnectionHandler {
    topic: String,
    channel: String,
    handler: Arc<dyn Handler + Send + Sync + 'static>,
    messages_received: Arc<AtomicU64>,
    messages_finished: Arc<AtomicU64>,
    messages_requeued: Arc<AtomicU64>,
    total_rdy_count: Arc<AtomicI32>,
    max_in_flight: Arc<AtomicI32>,
    disable_auto_response: bool,
}

impl ConnectionHandler {
    fn new(consumer: &Consumer) -> Self {
        Self {
            topic: consumer.topic.clone(),
            channel: consumer.channel.clone(),
            handler: consumer.handler.clone(),
            messages_received: Arc::new(AtomicU64::new(0)),
            messages_finished: Arc::new(AtomicU64::new(0)),
            messages_requeued: Arc::new(AtomicU64::new(0)),
            total_rdy_count: Arc::new(AtomicI32::new(0)),
            max_in_flight: Arc::new(AtomicI32::new(consumer.config.max_in_flight)),
            disable_auto_response: consumer.config.disable_auto_response,
        }
    }

    async fn handle_connection(&self, conn: Arc<Connection>) -> Result<()> {
        // 发送订阅命令
        let sub_cmd = Command::Subscribe(self.topic.clone(), self.channel.clone());
        conn.send_command(sub_cmd).await?;

        // 发送就绪命令
        let rdy_count = self.max_in_flight.load(Ordering::Relaxed);
        let rdy_cmd = Command::Ready(rdy_count as u32);
        conn.send_command(rdy_cmd).await?;

        // 创建心跳间隔
        let mut heartbeat_interval = tokio::time::interval(Duration::from_secs(30));

        loop {
            tokio::select! {
                // 主动心跳检测
                _ = heartbeat_interval.tick() => {
                    if let Err(e) = conn.handle_heartbeat().await {
                        error!("心跳检测失败: {}", e);
                        return Err(e);
                    }
                }
                // 接收并处理消息
                frame = conn.read_frame() =>
                    match frame {
                        Ok(Frame::Response(data)) => {
                            // 检查是否是心跳消息
                            if data == b"_heartbeat_"
                                && let Err(e) = conn.send_command(Command::Nop).await {
                                    error!("发送心跳响应失败: {}", e);
                                    return Err(e);
                                }
                        }
                        Ok(Frame::Error(data)) => {
                            error!("NSQ错误: {:?}", String::from_utf8_lossy(&data));
                            // 如果是致命错误，需要重新连接
                            if String::from_utf8_lossy(&data).contains("E_INVALID") {
                                return Err(Error::Protocol(ProtocolError::Other(
                                    String::from_utf8_lossy(&data).to_string()
                                )));
                            }
                        }
                        Ok(Frame::Message(msg)) => {
                            self.messages_received.fetch_add(1, Ordering::Relaxed);

                            // 为消息附加连接引用（用于手动确认）
                            let msg_with_conn = msg.with_responder(Arc::clone(&conn));

                            // 处理消息
                            match self.handler.handle_message(msg_with_conn.clone()).await {
                                Ok(_) => {
                                    // 检查是否需要自动响应
                                    if !self.disable_auto_response && !msg_with_conn.is_auto_response_disabled() && !msg_with_conn.has_responded() {
                                        // 自动发送 FIN
                                        let msg_id = msg_with_conn.id_string();
                                        let fin_cmd = Command::Finish(msg_id);
                                        if let Err(e) = conn.send_command(fin_cmd).await {
                                            error!("发送 FIN 命令失败: {}", e);
                                            return Err(e);
                                        } else {
                                            self.messages_finished.fetch_add(1, Ordering::Relaxed);
                                        }
                                    } else if msg_with_conn.has_responded() {
                                        // 消息已经手动响应过了，更新统计
                                        self.messages_finished.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                                Err(e) => {
                                    error!("消息处理失败: {}", e);

                                    // 检查是否需要自动响应
                                    if !self.disable_auto_response && !msg_with_conn.is_auto_response_disabled() && !msg_with_conn.has_responded() {
                                        // 自动发送 REQ
                                        let msg_id = msg_with_conn.id_string();
                                        let req_cmd = Command::Requeue(msg_id, 0);
                                        if let Err(e) = conn.send_command(req_cmd).await {
                                            error!("发送 REQ 命令失败: {}", e);
                                            return Err(e);
                                        } else {
                                            self.messages_requeued.fetch_add(1, Ordering::Relaxed);
                                        }
                                    } else if msg_with_conn.has_responded() {
                                        // 消息已经手动响应过了，更新统计
                                        self.messages_requeued.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            }

                            // 更新 RDY 计数
                            let current_rdy = self.total_rdy_count.fetch_sub(1, Ordering::Relaxed);
                            if current_rdy <= self.max_in_flight.load(Ordering::Relaxed) / 2 {
                                let new_rdy = self.max_in_flight.load(Ordering::Relaxed);
                                let rdy_cmd = Command::Ready(new_rdy as u32);
                                if let Err(e) = conn.send_command(rdy_cmd).await {
                                    error!("发送 RDY 命令失败: {}", e);
                                    return Err(e);
                                } else {
                                    self.total_rdy_count.store(new_rdy, Ordering::Relaxed);
                                }
                            }
                        }
                        Err(e) => {
                            error!("读取帧失败: {}", e);
                            return Err(e);
                        }
                    }
            }
        }
    }
}

impl Consumer {
    pub fn new(
        topic: String,
        channel: String,
        config: ConsumerConfig,
        handler: impl Handler,
    ) -> Result<Self> {
        if !Self::is_valid_topic_name(&topic) {
            return Err(Error::Other(format!("Invalid topic name: {}", topic)));
        }
        if !Self::is_valid_channel_name(&channel) {
            return Err(Error::Other(format!("Invalid channel name: {}", channel)));
        }

        let (stop_tx, _) = mpsc::channel(1);

        Ok(Consumer {
            topic,
            channel,
            config: config.clone(),
            handler: Arc::new(handler),
            messages_received: AtomicU64::new(0),
            messages_finished: AtomicU64::new(0),
            messages_requeued: AtomicU64::new(0),
            connections: Arc::new(Mutex::new(HashMap::new())),
            total_rdy_count: AtomicI32::new(0),
            max_in_flight: AtomicI32::new(config.max_in_flight),
            is_running: AtomicBool::new(false),
            stop_chan: stop_tx,
        })
    }

    fn is_valid_topic_name(topic: &str) -> bool {
        if topic.is_empty() || topic.len() > 64 {
            return false;
        }
        topic
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
    }

    fn is_valid_channel_name(channel: &str) -> bool {
        if channel.is_empty() || channel.len() > 64 {
            return false;
        }
        channel.chars().all(|c| {
            c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' || c == '#' || c == '*'
        })
    }

    pub fn stats(&self) -> ConsumerStats {
        ConsumerStats {
            messages_received: self.messages_received.load(Ordering::Relaxed),
            messages_finished: self.messages_finished.load(Ordering::Relaxed),
            messages_requeued: self.messages_requeued.load(Ordering::Relaxed),
            connections: self.connections.blocking_lock().len() as i32,
        }
    }

    pub async fn connect_to_nsqd(&self, addr: String) -> Result<()> {
        let mut conns = self.connections.lock().await;
        if conns.contains_key(&addr) {
            return Ok(());
        }

        let conn = Arc::new(
            Connection::new(
                &addr,
                None,
                None,
                self.config.read_timeout,
                self.config.write_timeout,
            )
            .await?,
        );

        let conn_clone = Arc::clone(&conn);
        let handler = Arc::new(ConnectionHandler::new(self));
        let addr_clone = addr.clone();
        let config_clone = self.config.clone();

        // 启动消息处理循环
        tokio::spawn(async move {
            // 初始重试延迟（秒）
            let mut retry_delay = 1;
            // 最大重试延迟（秒）
            let max_retry_delay = 60;
            // 重试计数
            let mut retry_count = 0;

            loop {
                match handler.handle_connection(Arc::clone(&conn_clone)).await {
                    Ok(_) => {
                        info!("连接循环正常结束");
                        break;
                    }
                    Err(e) => {
                        retry_count += 1;
                        let is_connection_error = matches!(e,
                            Error::Io(ref io_err) if io_err.kind() == std::io::ErrorKind::BrokenPipe
                            || io_err.kind() == std::io::ErrorKind::ConnectionReset
                            || io_err.kind() == std::io::ErrorKind::ConnectionAborted
                            || io_err.kind() == std::io::ErrorKind::UnexpectedEof
                        ) || e.to_string().contains("early eof");

                        // 根据错误类型决定是否需要重连
                        if is_connection_error || matches!(e, Error::Timeout(_)) {
                            error!("连接错误 (尝试 #{}) 到 {}: {}", retry_count, addr_clone, e);

                            // 指数退避策略
                            let sleep_duration = if config_clone.backoff_strategy {
                                let jitter = rand::random::<f32>() * 0.3;
                                let delay = (retry_delay as f32 * (1.0 + jitter)) as u64;
                                retry_delay = std::cmp::min(retry_delay * 2, max_retry_delay);
                                delay
                            } else {
                                retry_delay
                            };

                            info!("将在 {}秒 后尝试重新连接到 {}", sleep_duration, addr_clone);
                            tokio::time::sleep(Duration::from_secs(sleep_duration)).await;

                            // 尝试重新建立连接
                            match conn_clone.reconnect().await {
                                Ok(_) => {
                                    // 重置重试计数和延迟
                                    info!("成功重新连接到 {}", addr_clone);
                                    retry_delay = 1;
                                    retry_count = 0;
                                }
                                Err(conn_err) => {
                                    error!("重新连接失败: {}", conn_err);
                                    continue;
                                }
                            }
                        } else {
                            // 对于其他类型的错误，记录并中断
                            error!("非连接错误，停止重试: {}", e);
                            break;
                        }
                    }
                }
            }
        });

        conns.insert(addr, conn);
        Ok(())
    }

    pub async fn disconnect_from_nsqd(&self, addr: String) -> Result<()> {
        let mut conns = self.connections.lock().await;
        if let Some(conn) = conns.remove(&addr) {
            conn.close().await?;
        }
        Ok(())
    }

    pub async fn start(&self) -> Result<()> {
        self.is_running.store(true, Ordering::Relaxed);
        Ok(())
    }

    pub async fn stop(&self) -> Result<()> {
        info!("开始优雅关闭消费者...");
        self.is_running.store(false, Ordering::Relaxed);

        // 发送停止信号
        let _ = self.stop_chan.send(()).await;

        // 等待所有连接关闭或超时
        let shutdown_deadline = tokio::time::sleep(self.config.shutdown_timeout);
        tokio::pin!(shutdown_deadline);

        let mut conns = self.connections.lock().await;
        for (addr, conn) in conns.drain() {
            info!("正在关闭到 {} 的连接", addr);

            tokio::select! {
                _ = &mut shutdown_deadline => {
                    error!("关闭连接超时");
                    break;
                }
                result = conn.close() => {
                    if let Err(e) = result {
                        error!("关闭到 {} 的连接时出错: {}", addr, e);
                    } else {
                        info!("成功关闭到 {} 的连接", addr);
                    }
                }
            }
        }

        info!("消费者已关闭");
        Ok(())
    }

    pub async fn connect_to_nsqlookupd(&self, lookupd_url: String) -> Result<()> {
        info!("正在从 nsqlookupd 获取 nsqd 节点列表...");
        let nodes = crate::lookup::lookup_nodes(&lookupd_url, &self.topic).await?;

        for node in nodes {
            info!("发现 nsqd 节点: {}", node);
            if let Err(e) = self.connect_to_nsqd(node.clone()).await {
                error!("连接到 nsqd 节点 {} 失败: {}", node, e);
            }
        }

        // 启动定期更新节点的任务
        let consumer = self.clone();
        let lookupd_url = lookupd_url.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(consumer.config.lookup_poll_interval);
            loop {
                interval.tick().await;
                match crate::lookup::lookup_nodes(&lookupd_url, &consumer.topic).await {
                    Ok(nodes) => {
                        for node in nodes {
                            if let Err(e) = consumer.connect_to_nsqd(node.clone()).await {
                                error!("连接到 nsqd 节点 {} 失败: {}", node, e);
                            }
                        }
                    }
                    Err(e) => {
                        error!("从 nsqlookupd 获取节点列表失败: {}", e);
                    }
                }
            }
        });

        Ok(())
    }
}

impl Clone for Consumer {
    fn clone(&self) -> Self {
        Consumer {
            topic: self.topic.clone(),
            channel: self.channel.clone(),
            config: self.config.clone(),
            handler: self.handler.clone(),
            messages_received: AtomicU64::new(self.messages_received.load(Ordering::Relaxed)),
            messages_finished: AtomicU64::new(self.messages_finished.load(Ordering::Relaxed)),
            messages_requeued: AtomicU64::new(self.messages_requeued.load(Ordering::Relaxed)),
            connections: self.connections.clone(),
            total_rdy_count: AtomicI32::new(self.total_rdy_count.load(Ordering::Relaxed)),
            max_in_flight: AtomicI32::new(self.max_in_flight.load(Ordering::Relaxed)),
            is_running: AtomicBool::new(self.is_running.load(Ordering::Relaxed)),
            stop_chan: self.stop_chan.clone(),
        }
    }
}
