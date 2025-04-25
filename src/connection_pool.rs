use crate::connection::Connection;
use crate::error::Result;
use crate::protocol::IdentifyConfig;
use dashmap::DashMap;
use log::{info, warn};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 连接池配置
#[derive(Debug, Clone)]
pub struct ConnectionPoolConfig {
    /// 连接超时时间
    pub connection_timeout: Duration,
    /// 最大空闲时间，超过此时间的连接会被清理
    pub max_idle_time: Duration,
    /// 健康检查间隔
    pub health_check_interval: Duration,
    /// 每个地址最大连接数
    pub max_connections_per_host: usize,
}

impl Default for ConnectionPoolConfig {
    fn default() -> Self {
        Self {
            connection_timeout: Duration::from_secs(5),
            max_idle_time: Duration::from_secs(60),
            health_check_interval: Duration::from_secs(30),
            max_connections_per_host: 10,
        }
    }
}

/// 连接健康状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    /// 未知状态
    Unknown,
    /// 健康状态
    Healthy,
    /// 不健康状态
    Unhealthy,
}

/// 池化连接
#[derive(Debug)]
pub struct PooledConnection {
    /// 连接实例
    pub connection: Arc<Connection>,
    /// 上次使用时间
    pub last_used: Instant,
    /// 上次检查时间
    pub last_checked: Instant,
    /// 健康状态
    pub health_status: HealthStatus,
    /// 连接指纹，用于标识连接
    pub fingerprint: String,
}

impl PooledConnection {
    /// 创建新的池化连接
    pub fn new(connection: Arc<Connection>, fingerprint: String) -> Self {
        let now = Instant::now();
        Self {
            connection,
            last_used: now,
            last_checked: now,
            health_status: HealthStatus::Unknown,
            fingerprint,
        }
    }

    /// 检查连接健康状态
    pub async fn check_health(&mut self) -> bool {
        self.last_checked = Instant::now();

        match self.connection.handle_heartbeat().await {
            Ok(_) => {
                self.health_status = HealthStatus::Healthy;
                true
            }
            Err(e) => {
                warn!("连接健康检查失败: {}, 地址: {}", e, self.connection.addr());
                self.health_status = HealthStatus::Unhealthy;
                false
            }
        }
    }

    /// 更新最后使用时间
    pub fn update_last_used(&mut self) {
        self.last_used = Instant::now();
    }

    /// 检查连接是否空闲超时
    pub fn is_idle(&self, max_idle_time: Duration) -> bool {
        self.last_used.elapsed() > max_idle_time
    }
}

/// 连接池管理器
#[derive(Debug)]
pub struct ConnectionPool {
    /// 连接池配置
    config: ConnectionPoolConfig,
    /// 连接存储，使用DashMap提供高效的并发访问
    connections: DashMap<String, Vec<PooledConnection>>,
}

impl ConnectionPool {
    /// 创建新的连接池
    pub fn new(config: ConnectionPoolConfig) -> Self {
        Self {
            config,
            connections: DashMap::new(),
        }
    }

    /// 生成连接指纹
    fn generate_fingerprint(
        addr: &str,
        identify_config: &Option<IdentifyConfig>,
        auth_secret: &Option<String>,
    ) -> String {
        // 简单实现，实际应用中可能需要更复杂的指纹生成算法
        format!(
            "{}:{}:{}",
            addr,
            identify_config
                .as_ref()
                .map(|c| format!("{:?}", c))
                .unwrap_or_default(),
            auth_secret.as_ref().unwrap_or(&String::new())
        )
    }

    /// 获取连接
    pub async fn get_connection(
        &self,
        addr: &str,
        identify_config: Option<IdentifyConfig>,
        auth_secret: Option<String>,
    ) -> Result<Arc<Connection>> {
        let fingerprint = Self::generate_fingerprint(addr, &identify_config, &auth_secret);

        // 尝试从连接池获取健康的连接
        if let Some(mut connections) = self.connections.get_mut(&fingerprint) {
            // 查找健康的连接
            for conn in connections.value_mut().iter_mut() {
                if conn.health_status != HealthStatus::Unhealthy {
                    conn.update_last_used();

                    return Ok(Arc::clone(&conn.connection));
                }
            }

            // 尝试恢复一个不健康的连接
            for conn in connections.value_mut().iter_mut() {
                if conn.check_health().await {
                    conn.update_last_used();

                    return Ok(Arc::clone(&conn.connection));
                }
            }

            // 如果连接数未达到上限，创建新连接
            if connections.value().len() < self.config.max_connections_per_host {
                return self
                    .create_and_store_connection(addr, identify_config, auth_secret, &fingerprint)
                    .await;
            }

            // 连接池已满，找到最久未使用的连接并替换
            if let Some(oldest_index) = connections
                .value()
                .iter()
                .enumerate()
                .min_by_key(|(_, conn)| conn.last_used)
                .map(|(i, _)| i)
            {
                connections.value_mut().remove(oldest_index);
                return self
                    .create_and_store_connection(addr, identify_config, auth_secret, &fingerprint)
                    .await;
            }
        }

        // 如果连接池中没有该地址的连接，创建新连接
        self.create_and_store_connection(addr, identify_config, auth_secret, &fingerprint)
            .await
    }

    /// 创建并存储连接
    async fn create_and_store_connection(
        &self,
        addr: &str,
        identify_config: Option<IdentifyConfig>,
        auth_secret: Option<String>,
        fingerprint: &str,
    ) -> Result<Arc<Connection>> {
        // 创建新连接
        let connection = Connection::new(
            addr,
            identify_config.clone(),
            auth_secret.clone(),
            Duration::from_secs(60), // 默认读超时
            Duration::from_secs(5),  // 默认写超时
        )
        .await?;

        let connection = Arc::new(connection);
        let pooled_connection =
            PooledConnection::new(Arc::clone(&connection), fingerprint.to_string());

        // 添加到连接池
        self.connections
            .entry(fingerprint.to_string())
            .or_insert_with(Vec::new)
            .push(pooled_connection);

        Ok(connection)
    }

    /// 启动连接池清理任务
    pub fn start_cleanup_task(pool: Arc<ConnectionPool>) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            loop {
                interval.tick().await;
                pool.cleanup_idle_connections().await;
            }
        });
    }

    /// 启动健康检查任务
    pub fn start_health_check_task(pool: Arc<ConnectionPool>) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(pool.config.health_check_interval);
            loop {
                interval.tick().await;
                pool.check_connections_health().await;
            }
        });
    }

    /// 清理空闲连接
    pub async fn cleanup_idle_connections(&self) {
        let max_idle_time = self.config.max_idle_time;

        for mut entry in self.connections.iter_mut() {
            let before_count = entry.value().len();
            entry
                .value_mut()
                .retain(|conn| !conn.is_idle(max_idle_time));
            let after_count = entry.value().len();

            if before_count > after_count {}
        }
    }

    /// 检查连接健康状态
    pub async fn check_connections_health(&self) {
        for mut entry in self.connections.iter_mut() {
            for conn in entry.value_mut().iter_mut() {
                // 只检查超过健康检查间隔的连接
                if conn.last_checked.elapsed() > self.config.health_check_interval {
                    let _ = conn.check_health().await;
                }
            }
        }
    }

    /// 获取连接池统计信息
    pub fn get_stats(&self) -> ConnectionPoolStats {
        let mut stats = ConnectionPoolStats::default();

        for entry in self.connections.iter() {
            stats.total_connections += entry.value().len();

            for conn in entry.value() {
                match conn.health_status {
                    HealthStatus::Healthy => stats.healthy_connections += 1,
                    HealthStatus::Unhealthy => stats.unhealthy_connections += 1,
                    HealthStatus::Unknown => stats.unknown_status_connections += 1,
                }

                if conn.is_idle(self.config.max_idle_time) {
                    stats.idle_connections += 1;
                }
            }
        }

        stats.host_count = self.connections.len();
        stats
    }
}

/// 连接池统计信息
#[derive(Debug, Default, Clone)]
pub struct ConnectionPoolStats {
    /// 总连接数
    pub total_connections: usize,
    /// 健康连接数
    pub healthy_connections: usize,
    /// 不健康连接数
    pub unhealthy_connections: usize,
    /// 未知状态连接数
    pub unknown_status_connections: usize,
    /// 空闲连接数
    pub idle_connections: usize,
    /// 主机数量
    pub host_count: usize,
}

/// 创建一个新的连接池实例
pub fn create_connection_pool(config: ConnectionPoolConfig) -> Arc<ConnectionPool> {
    let pool = Arc::new(ConnectionPool::new(config));

    // 启动清理和健康检查任务
    ConnectionPool::start_cleanup_task(Arc::clone(&pool));
    ConnectionPool::start_health_check_task(Arc::clone(&pool));

    info!("NSQ连接池已初始化");
    pool
}

/// 创建默认配置的连接池实例
pub fn create_default_connection_pool() -> Arc<ConnectionPool> {
    create_connection_pool(ConnectionPoolConfig::default())
}
