use async_trait::async_trait;
use log::{error, info};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::sync::Mutex;

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
                            if data == b"_heartbeat_" {
                                if let Err(e) = conn.send_command(Command::Nop).await {
                                    error!("发送心跳响应失败: {}", e);
                                    return Err(e);
                                }
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

                            // 处理消息
                            match self.handler.handle_message(msg.clone()).await {
                                Ok(_) => {
                                    let msg_id = String::from_utf8_lossy(&msg.id).to_string();
                                    let fin_cmd = Command::Finish(msg_id);
                                    if let Err(e) = conn.send_command(fin_cmd).await {
                                        error!("发送 FIN 命令失败: {}", e);
                                        return Err(e);
                                    } else {
                                        self.messages_finished.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                                Err(e) => {
                                    error!("消息处理失败: {}", e);
                                    let msg_id = String::from_utf8_lossy(&msg.id).to_string();
                                    let req_cmd = Command::Requeue(msg_id, 0);
                                    if let Err(e) = conn.send_command(req_cmd).await {
                                        error!("发送 REQ 命令失败: {}", e);
                                        return Err(e);
                                    } else {
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
        handler: impl Handler + Send + Sync + 'static,
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

        // 启动消息处理循环
        tokio::spawn(async move {
            loop {
                match handler.handle_connection(Arc::clone(&conn_clone)).await {
                    Ok(_) => {
                        info!("连接循环正常结束");
                        break;
                    }
                    Err(e) => {
                        error!("连接循环错误: {}", e);
                        // 等待一段时间后重试
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        info!("正在尝试重新连接到 {}", addr_clone);
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
