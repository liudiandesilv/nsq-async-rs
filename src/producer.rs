use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use backoff::ExponentialBackoffBuilder;
use log::debug;
use tokio::sync::RwLock;

use crate::commands::{create_nsqd_connection, lookup_nsqd_nodes};
use crate::connection::Connection;
use crate::connection_pool::ConnectionPool;
use crate::error::{Error, Result};
use crate::protocol::{Command, Frame, IdentifyConfig, ProtocolError};

/// 生产者配置
#[derive(Debug, Clone)]
pub struct ProducerConfig {
    /// NSQ服务器地址
    pub nsqd_addresses: Vec<String>,
    /// NSQ查询服务地址
    pub nsqlookupd_addresses: Vec<String>,
    /// 连接超时时间
    pub connection_timeout: Duration,
    /// 认证密钥
    pub auth_secret: Option<String>,
    /// 身份配置
    pub identify_config: Option<IdentifyConfig>,
    /// 重连策略
    pub backoff_config: BackoffConfig,
}

impl Default for ProducerConfig {
    fn default() -> Self {
        Self {
            nsqd_addresses: vec![],
            nsqlookupd_addresses: vec![],
            connection_timeout: Duration::from_secs(5),
            auth_secret: None,
            identify_config: None,
            backoff_config: BackoffConfig::default(),
        }
    }
}

/// 重连策略配置
#[derive(Debug, Clone)]
pub struct BackoffConfig {
    /// 初始间隔
    pub initial_interval: Duration,
    /// 最大间隔
    pub max_interval: Duration,
    /// 倍数
    pub multiplier: f64,
    /// 最大重试时间
    pub max_elapsed_time: Option<Duration>,
}

impl Default for BackoffConfig {
    fn default() -> Self {
        Self {
            initial_interval: Duration::from_millis(100),
            max_interval: Duration::from_secs(10),
            multiplier: 2.0,
            max_elapsed_time: Some(Duration::from_secs(60)),
        }
    }
}

/// NSQ生产者特性
#[async_trait]
pub trait Producer: Send + Sync {
    /// 向NSQ发布消息
    async fn publish<T: AsRef<[u8]> + Send + Sync>(&self, topic: &str, message: T) -> Result<()>;

    /// 向NSQ发布延迟消息
    async fn publish_delayed<T: AsRef<[u8]> + Send + Sync>(
        &self,
        topic: &str,
        message: T,
        delay: Duration,
    ) -> Result<()>;

    /// 批量发布消息
    async fn publish_multi<T: AsRef<[u8]> + Send + Sync>(
        &self,
        topic: &str,
        messages: Vec<T>,
    ) -> Result<()>;

    /// 设置外部连接池 - 返回原始类型，不能直接使用
    fn with_connection_pool(self, _pool: Arc<ConnectionPool>) -> Self
    where
        Self: Sized,
    {
        self
    }
}

/// NSQ生产者实现
pub struct NsqProducer {
    /// 生产者配置
    config: ProducerConfig,
    /// 内部连接池
    connections: RwLock<HashMap<String, Arc<Connection>>>,
    /// 外部连接池（可选）
    external_pool: Option<Arc<ConnectionPool>>,
}

impl NsqProducer {
    /// 创建新的NSQ生产者
    pub fn new(config: ProducerConfig) -> Self {
        Self {
            config,
            connections: RwLock::new(HashMap::new()),
            external_pool: None,
        }
    }

    /// 设置外部连接池
    pub fn with_connection_pool(mut self, pool: Arc<ConnectionPool>) -> Self {
        self.external_pool = Some(pool);
        self
    }

    /// 获取或创建到NSQ服务器的连接
    async fn get_or_create_connection(&self, addr: &str) -> Result<Arc<Connection>> {
        // 优先使用外部连接池（如果提供了）
        if let Some(pool) = &self.external_pool {
            debug!("使用外部连接池获取连接: {}", addr);
            return pool
                .get_connection(
                    addr,
                    self.config.identify_config.clone(),
                    self.config.auth_secret.clone(),
                )
                .await;
        }

        // 尝试从内部连接池获取连接
        let connections = self.connections.read().await;
        if let Some(connection) = connections.get(addr) {
            return Ok(connection.clone());
        }
        drop(connections);

        // 创建新连接
        debug!("为地址 {} 创建新连接", addr);
        let connection = create_nsqd_connection(
            addr,
            self.config.identify_config.clone(),
            self.config.auth_secret.clone(),
        )
        .await?;

        // 添加连接到内部连接池
        let mut connections = self.connections.write().await;
        connections.insert(addr.to_string(), connection.clone());

        Ok(connection)
    }

    /// 获取用于发布消息的连接
    async fn get_publish_connection(&self, topic: &str) -> Result<Arc<Connection>> {
        // 如果直接配置了nsqd地址，使用第一个
        if !self.config.nsqd_addresses.is_empty() {
            return self
                .get_or_create_connection(&self.config.nsqd_addresses[0])
                .await;
        }

        // 如果配置了nsqlookupd，使用它查找nsqd
        if !self.config.nsqlookupd_addresses.is_empty() {
            let addr = &self.config.nsqlookupd_addresses[0];
            let nodes = lookup_nsqd_nodes(addr, topic).await?;

            if nodes.is_empty() {
                return Err(Error::Connection(format!(
                    "nsqlookupd未找到主题 {} 的生产者",
                    topic
                )));
            }

            return self.get_or_create_connection(&nodes[0]).await;
        }

        Err(Error::Config("未配置nsqd或nsqlookupd地址".to_string()))
    }
}

#[async_trait]
impl Producer for NsqProducer {
    async fn publish<T: AsRef<[u8]> + Send + Sync>(&self, topic: &str, message: T) -> Result<()> {
        let backoff = ExponentialBackoffBuilder::new()
            .with_initial_interval(self.config.backoff_config.initial_interval)
            .with_max_interval(self.config.backoff_config.max_interval)
            .with_multiplier(self.config.backoff_config.multiplier)
            .with_max_elapsed_time(self.config.backoff_config.max_elapsed_time)
            .build();

        let topic_owned = topic.to_string();
        let message_bytes = message.as_ref().to_vec();

        let result = backoff::future::retry(backoff, || async {
            let connection = match self.get_publish_connection(&topic_owned).await {
                Ok(conn) => conn,
                Err(e) => return Err(backoff::Error::permanent(e)),
            };

            let cmd = Command::Publish(topic_owned.clone(), message_bytes.clone());
            match connection.send_command(cmd).await {
                Ok(_) => {
                    // 读取响应
                    match connection.read_frame().await {
                        Ok(Frame::Response(_)) => Ok(()),
                        Ok(Frame::Error(data)) => {
                            let error_msg = String::from_utf8_lossy(&data);
                            Err(backoff::Error::transient(Error::Protocol(
                                ProtocolError::Other(error_msg.to_string()),
                            )))
                        }
                        Ok(_) => Err(backoff::Error::transient(Error::Protocol(
                            ProtocolError::Other("收到意外响应".to_string()),
                        ))),
                        Err(e) => Err(backoff::Error::transient(e)),
                    }
                }
                Err(e) => Err(backoff::Error::transient(e)),
            }
        })
        .await;

        match result {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }

    async fn publish_delayed<T: AsRef<[u8]> + Send + Sync>(
        &self,
        topic: &str,
        message: T,
        delay: Duration,
    ) -> Result<()> {
        // 初始化退避策略
        let backoff = ExponentialBackoffBuilder::new()
            .with_initial_interval(self.config.backoff_config.initial_interval)
            .with_max_interval(self.config.backoff_config.max_interval)
            .with_multiplier(self.config.backoff_config.multiplier)
            .with_max_elapsed_time(self.config.backoff_config.max_elapsed_time)
            .build();

        let topic_owned = topic.to_string();
        let message_bytes = message.as_ref().to_vec();

        let result = backoff::future::retry(backoff, || async {
            let connection = match self.get_publish_connection(&topic_owned).await {
                Ok(conn) => conn,
                Err(e) => return Err(backoff::Error::permanent(e)),
            };

            let cmd = Command::DelayedPublish(
                topic_owned.clone(),
                message_bytes.clone(),
                delay.as_millis() as u32,
            );
            match connection.send_command(cmd).await {
                Ok(_) => {
                    // 读取响应
                    match connection.read_frame().await {
                        Ok(Frame::Response(_)) => Ok(()),
                        Ok(Frame::Error(data)) => {
                            let error_msg = String::from_utf8_lossy(&data);
                            Err(backoff::Error::transient(Error::Protocol(
                                ProtocolError::Other(error_msg.to_string()),
                            )))
                        }
                        Ok(_) => Err(backoff::Error::transient(Error::Protocol(
                            ProtocolError::Other("收到意外响应".to_string()),
                        ))),
                        Err(e) => Err(backoff::Error::transient(e)),
                    }
                }
                Err(e) => Err(backoff::Error::transient(e)),
            }
        })
        .await;

        match result {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }

    async fn publish_multi<T: AsRef<[u8]> + Send + Sync>(
        &self,
        topic: &str,
        messages: Vec<T>,
    ) -> Result<()> {
        if messages.is_empty() {
            debug!("忽略空消息列表");
            return Ok(());
        }

        // 将消息转换为字节向量
        let byte_messages: Vec<Vec<u8>> =
            messages.iter().map(|msg| msg.as_ref().to_vec()).collect();

        // 使用与批量发送相同的逻辑处理
        let backoff = ExponentialBackoffBuilder::new()
            .with_initial_interval(self.config.backoff_config.initial_interval)
            .with_max_interval(self.config.backoff_config.max_interval)
            .with_multiplier(self.config.backoff_config.multiplier)
            .with_max_elapsed_time(self.config.backoff_config.max_elapsed_time)
            .build();

        let topic_owned = topic.to_string();

        let result = backoff::future::retry(backoff, || async {
            let connection = match self.get_publish_connection(&topic_owned).await {
                Ok(conn) => conn,
                Err(e) => return Err(backoff::Error::permanent(e)),
            };

            let cmd = Command::Mpublish(topic_owned.clone(), byte_messages.clone());
            match connection.send_command(cmd).await {
                Ok(_) => {
                    // 读取响应
                    match connection.read_frame().await {
                        Ok(Frame::Response(_)) => Ok(()),
                        Ok(Frame::Error(data)) => {
                            let error_msg = String::from_utf8_lossy(&data);
                            Err(backoff::Error::transient(Error::Protocol(
                                ProtocolError::Other(error_msg.to_string()),
                            )))
                        }
                        Ok(_) => Err(backoff::Error::transient(Error::Protocol(
                            ProtocolError::Other("收到意外响应".to_string()),
                        ))),
                        Err(e) => Err(backoff::Error::transient(e)),
                    }
                }
                Err(e) => Err(backoff::Error::transient(e)),
            }
        })
        .await;

        match result {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

impl NsqProducer {
    /// 获取生产者配置
    pub fn config(&self) -> &ProducerConfig {
        &self.config
    }
}

/// 创建一个新的NSQ生产者
pub fn new_producer(config: ProducerConfig) -> NsqProducer {
    NsqProducer::new(config)
}
