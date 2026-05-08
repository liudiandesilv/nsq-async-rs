use async_trait::async_trait;
use log::{error, info};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::time::Duration;
use thiserror::Error;
use tokio::sync::{Mutex, Semaphore};

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
    /// 并发 handler 数量上限，控制同时处理的消息数
    /// 默认与 max_in_flight 相同
    pub concurrent_handlers: usize,
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
            concurrent_handlers: 1,
        }
    }
}

pub struct Consumer {
    topic: String,
    channel: String,
    config: ConsumerConfig,
    handler: Arc<dyn Handler + Send + Sync + 'static>,

    // Stats
    messages_received: Arc<AtomicU64>,
    messages_finished: Arc<AtomicU64>,
    messages_requeued: Arc<AtomicU64>,

    // Connection management
    connections: Arc<Mutex<HashMap<String, Arc<Connection>>>>,
    connection_count: Arc<AtomicI32>,
    max_in_flight: Arc<AtomicI32>,

    // Control
    is_running: Arc<AtomicBool>,
}

struct ConnectionHandler {
    topic: String,
    channel: String,
    handler: Arc<dyn Handler + Send + Sync + 'static>,
    messages_received: Arc<AtomicU64>,
    messages_finished: Arc<AtomicU64>,
    messages_requeued: Arc<AtomicU64>,
    max_in_flight: Arc<AtomicI32>,
    is_running: Arc<AtomicBool>,
    disable_auto_response: bool,
    semaphore: Arc<Semaphore>,
}

impl ConnectionHandler {
    fn new(consumer: &Consumer) -> Self {
        let concurrent = consumer.config.concurrent_handlers.max(1);
        Self {
            topic: consumer.topic.clone(),
            channel: consumer.channel.clone(),
            handler: consumer.handler.clone(),
            messages_received: Arc::clone(&consumer.messages_received),
            messages_finished: Arc::clone(&consumer.messages_finished),
            messages_requeued: Arc::clone(&consumer.messages_requeued),
            max_in_flight: Arc::clone(&consumer.max_in_flight),
            is_running: Arc::clone(&consumer.is_running),
            disable_auto_response: consumer.config.disable_auto_response,
            semaphore: Arc::new(Semaphore::new(concurrent)),
        }
    }

    async fn handle_connection(&self, conn: Arc<Connection>) -> Result<()> {
        let sub_cmd = Command::Subscribe(self.topic.clone(), self.channel.clone());
        conn.send_command(sub_cmd).await?;

        let total_rdy_count = Arc::new(AtomicI32::new(0));
        let rdy_count = self.max_in_flight.load(Ordering::Relaxed);
        conn.send_command(Command::Ready(rdy_count as u32)).await?;
        total_rdy_count.store(rdy_count, Ordering::Relaxed);

        let mut heartbeat_interval = tokio::time::interval(Duration::from_secs(30));

        loop {
            if !self.is_running.load(Ordering::Relaxed) {
                info!("消费者已停止，结束连接处理循环");
                return Ok(());
            }

            tokio::select! {
                _ = heartbeat_interval.tick() => {
                    if let Err(e) = conn.handle_heartbeat().await {
                        error!("心跳检测失败: {}", e);
                        return Err(e);
                    }
                }
                frame = conn.read_frame() => match frame {
                    Ok(Frame::Response(data)) => {
                        if data == b"_heartbeat_" {
                            if let Err(e) = conn.send_command(Command::Nop).await {
                                error!("发送心跳响应失败: {}", e);
                                return Err(e);
                            }
                        }
                    }
                    Ok(Frame::Error(data)) => {
                        error!("NSQ错误: {:?}", String::from_utf8_lossy(&data));
                        if String::from_utf8_lossy(&data).contains("E_INVALID") {
                            return Err(Error::Protocol(ProtocolError::Other(
                                String::from_utf8_lossy(&data).to_string(),
                            )));
                        }
                    }
                    Ok(Frame::Message(msg)) => {
                        self.messages_received.fetch_add(1, Ordering::Relaxed);
                        let msg_with_conn = msg.with_responder(Arc::clone(&conn));

                        let permit = Arc::clone(&self.semaphore)
                            .acquire_owned()
                            .await
                            .map_err(|_| Error::Other("semaphore closed".to_string()))?;

                        let handler = Arc::clone(&self.handler);
                        let conn_write = Arc::clone(&conn);
                        let messages_finished = Arc::clone(&self.messages_finished);
                        let messages_requeued = Arc::clone(&self.messages_requeued);
                        let disable_auto_response = self.disable_auto_response;
                        let max_in_flight = self.max_in_flight.load(Ordering::Relaxed);
                        let rdy_ref = Arc::clone(&total_rdy_count);

                        tokio::spawn(async move {
                            let _permit = permit;

                            match handler.handle_message(msg_with_conn.clone()).await {
                                Ok(_) => {
                                    if !disable_auto_response
                                        && !msg_with_conn.is_auto_response_disabled()
                                        && !msg_with_conn.has_responded()
                                    {
                                        if let Err(e) = conn_write
                                            .send_command(Command::Finish(msg_with_conn.id_string()))
                                            .await
                                        {
                                            error!("发送 FIN 命令失败: {}", e);
                                            return;
                                        }
                                        messages_finished.fetch_add(1, Ordering::Relaxed);
                                    } else if msg_with_conn.has_responded() {
                                        messages_finished.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                                Err(e) => {
                                    error!("消息处理失败: {}", e);
                                    if !disable_auto_response
                                        && !msg_with_conn.is_auto_response_disabled()
                                        && !msg_with_conn.has_responded()
                                    {
                                        if let Err(e) = conn_write
                                            .send_command(Command::Requeue(
                                                msg_with_conn.id_string(),
                                                0,
                                            ))
                                            .await
                                        {
                                            error!("发送 REQ 命令失败: {}", e);
                                            return;
                                        }
                                        messages_requeued.fetch_add(1, Ordering::Relaxed);
                                    } else if msg_with_conn.has_responded() {
                                        messages_requeued.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                            }

                            // handler 完成后补充 RDY
                            let remaining = rdy_ref.fetch_sub(1, Ordering::Relaxed) - 1;
                            if remaining <= max_in_flight / 2 {
                                if let Err(e) = conn_write
                                    .send_command(Command::Ready(max_in_flight as u32))
                                    .await
                                {
                                    error!("发送 RDY 命令失败: {}", e);
                                } else {
                                    rdy_ref.store(max_in_flight, Ordering::Relaxed);
                                }
                            }
                        });
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

        Ok(Consumer {
            topic,
            channel,
            config: config.clone(),
            handler: Arc::new(handler),
            messages_received: Arc::new(AtomicU64::new(0)),
            messages_finished: Arc::new(AtomicU64::new(0)),
            messages_requeued: Arc::new(AtomicU64::new(0)),
            connections: Arc::new(Mutex::new(HashMap::new())),
            connection_count: Arc::new(AtomicI32::new(0)),
            max_in_flight: Arc::new(AtomicI32::new(config.max_in_flight)),
            is_running: Arc::new(AtomicBool::new(true)),
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
            connections: self.connection_count.load(Ordering::Relaxed),
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
                if !handler.is_running.load(Ordering::Relaxed) {
                    info!("消费者已停止，结束到 {} 的重连循环", addr_clone);
                    break;
                }

                match handler.handle_connection(Arc::clone(&conn_clone)).await {
                    Ok(_) => {
                        info!("连接循环正常结束");
                        break;
                    }
                    Err(e) => {
                        retry_count += 1;
                        if !handler.is_running.load(Ordering::Relaxed) {
                            info!("消费者已停止，不再重连 {}", addr_clone);
                            break;
                        }

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

                            if !handler.is_running.load(Ordering::Relaxed) {
                                info!("消费者已停止，跳过重连 {}", addr_clone);
                                break;
                            }

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
        self.connection_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    pub async fn disconnect_from_nsqd(&self, addr: String) -> Result<()> {
        let mut conns = self.connections.lock().await;
        if let Some(conn) = conns.remove(&addr) {
            self.connection_count.fetch_sub(1, Ordering::Relaxed);
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
        self.connection_count.store(0, Ordering::Relaxed);

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

                if !consumer.is_running.load(Ordering::Relaxed) {
                    info!("消费者已停止，结束 nsqlookupd poll loop");
                    break;
                }

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
            messages_received: Arc::clone(&self.messages_received),
            messages_finished: Arc::clone(&self.messages_finished),
            messages_requeued: Arc::clone(&self.messages_requeued),
            connections: Arc::clone(&self.connections),
            connection_count: Arc::clone(&self.connection_count),
            max_in_flight: Arc::clone(&self.max_in_flight),
            is_running: Arc::clone(&self.is_running),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoopHandler;

    #[async_trait]
    impl Handler for NoopHandler {
        async fn handle_message(&self, _message: ProtocolMessage) -> Result<()> {
            Ok(())
        }
    }

    fn new_consumer() -> Consumer {
        Consumer::new(
            "test_topic".to_string(),
            "test_channel".to_string(),
            ConsumerConfig::default(),
            NoopHandler,
        )
        .unwrap()
    }

    #[test]
    fn connection_handler_reuses_consumer_shared_state() {
        let consumer = new_consumer();
        let handler = ConnectionHandler::new(&consumer);

        assert!(Arc::ptr_eq(
            &handler.messages_received,
            &consumer.messages_received
        ));
        assert!(Arc::ptr_eq(
            &handler.messages_finished,
            &consumer.messages_finished
        ));
        assert!(Arc::ptr_eq(
            &handler.messages_requeued,
            &consumer.messages_requeued
        ));
        assert!(Arc::ptr_eq(&handler.max_in_flight, &consumer.max_in_flight));
        assert!(Arc::ptr_eq(&handler.is_running, &consumer.is_running));
    }

    #[test]
    fn consumer_clone_shares_runtime_state() {
        let consumer = new_consumer();
        let cloned = consumer.clone();

        consumer.messages_received.store(3, Ordering::Relaxed);
        consumer.connection_count.store(2, Ordering::Relaxed);
        consumer.is_running.store(false, Ordering::Relaxed);

        assert_eq!(cloned.messages_received.load(Ordering::Relaxed), 3);
        assert_eq!(cloned.connection_count.load(Ordering::Relaxed), 2);
        assert!(!cloned.is_running.load(Ordering::Relaxed));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stats_reads_connection_count_in_async_context() {
        let consumer = new_consumer();
        consumer.connection_count.store(2, Ordering::Relaxed);

        let stats = consumer.stats();

        assert_eq!(stats.connections, 2);
    }
}
