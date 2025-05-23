use log::warn;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::{oneshot, Mutex};

/// 连接池错误
#[derive(Debug, Error)]
pub enum PoolError {
    #[error("连接池已关闭")]
    Closed,
    #[error("达到最大活跃连接数")]
    MaxActiveConnReached,
    #[error("连接为空")]
    NilConnection,
    #[error("创建连接失败: {0}")]
    CreateConnection(String),
    #[error("关闭连接失败: {0}")]
    CloseConnection(String),
    #[error("检查连接失败: {0}")]
    PingConnection(String),
}

/// 带有创建时间的连接包装
#[derive(Clone)]
pub struct PooledConn<T> {
    pub conn: Arc<T>,
    pub created_time: Instant,
}

pub type Result<T> = std::result::Result<T, PoolError>;

/// 连接池配置
#[derive(Clone)]
pub struct PoolConfig {
    pub initial_cap: usize,     // 初始连接数
    pub max_cap: usize,         // 最大连接数
    pub max_idle: usize,        // 最大空闲连接
    pub idle_timeout: Duration, // 空闲超时时间
    pub max_lifetime: Duration, // 连接最大生命周期
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            initial_cap: 5,
            max_cap: 20,
            max_idle: 10,
            idle_timeout: Duration::from_secs(30),
            max_lifetime: Duration::from_secs(300), // 5分钟的连接生命周期
        }
    }
}

/// 空闲连接包装
struct IdleConn<T> {
    conn: Arc<T>,
    idle_time: Instant,   // 空闲时间（最后一次归还时间）
    created_time: Instant, // 创建时间
}

/// 连接请求
struct ConnRequest<T> {
    sender: oneshot::Sender<Result<PooledConn<T>>>,
}

/// 通用连接池
pub struct Pool<T> {
    idle_conns: Mutex<VecDeque<IdleConn<T>>>, // 空闲连接队列
    config: PoolConfig,                       // 配置
    factory: Arc<dyn Fn() -> Result<T> + Send + Sync>, // 创建连接的工厂函数
    closer: Arc<dyn Fn(&T) -> std::result::Result<(), String> + Send + Sync>, // 关闭连接的函数
    pinger: Option<Arc<dyn Fn(&T) -> std::result::Result<(), String> + Send + Sync>>, // 检查连接有效性的函数
    opening_conns: Mutex<usize>,           // 当前打开的连接数
    closed: Mutex<bool>,                   // 连接池是否已关闭
    conn_reqs: Mutex<Vec<ConnRequest<T>>>, // 连接请求队列
}

impl<T: Send + Sync + 'static> Pool<T> {
    /// 创建新的连接池
    pub async fn new<F, C>(
        config: PoolConfig,
        factory: F,
        closer: C,
        pinger: Option<impl Fn(&T) -> std::result::Result<(), String> + Send + Sync + 'static>,
    ) -> Result<Self>
    where
        F: Fn() -> Result<T> + Send + Sync + 'static,
        C: Fn(&T) -> std::result::Result<(), String> + Send + Sync + 'static,
    {
        let pool = Self {
            idle_conns: Mutex::new(VecDeque::with_capacity(config.max_idle)),
            config,
            factory: Arc::new(factory),
            closer: Arc::new(closer),
            pinger: pinger.map(|p| {
                Arc::new(p) as Arc<dyn Fn(&T) -> std::result::Result<(), String> + Send + Sync>
            }),
            opening_conns: Mutex::new(0),
            closed: Mutex::new(false),
            conn_reqs: Mutex::new(Vec::new()),
        };

        // 预创建初始连接
        for _ in 0..pool.config.initial_cap {
            let conn = (pool.factory)()?;
            let mut conns = pool.idle_conns.lock().await;
            conns.push_back(IdleConn {
                conn: Arc::new(conn),
                idle_time: Instant::now(),
                created_time: Instant::now(), // 创建时记录创建时间
            });
            let mut count = pool.opening_conns.lock().await;
            *count += 1;
        }

        Ok(pool)
    }

    /// 清理超时连接
    pub async fn clean_idle_conns(&self) -> Result<()> {
        if *self.closed.lock().await {
            return Ok(());
        }

        let mut conns = self.idle_conns.lock().await;
        let mut count = self.opening_conns.lock().await;
        let mut removed = 0;

        // 从后向前遍历，这样删除时不会影响索引
        for i in (0..conns.len()).rev() {
            let idle_conn = &conns[i];
            if self.config.idle_timeout > Duration::from_secs(0)
                && idle_conn.idle_time.elapsed() > self.config.idle_timeout
            {
                // 关闭超时连接
                if let Err(e) = (self.closer)(&idle_conn.conn) {
                    return Err(PoolError::CloseConnection(e));
                }
                conns.remove(i);
                removed += 1;
            }
        }

        *count -= removed;
        Ok(())
    }

    /// 获取连接
    pub async fn get(&self) -> Result<PooledConn<T>> {
        if *self.closed.lock().await {
            return Err(PoolError::Closed);
        }

        // 先尝试从空闲连接中获取
        {
            let mut conns = self.idle_conns.lock().await;
            while let Some(idle_conn) = conns.pop_front() {
                // 检查是否超时
                // 检查空闲超时
                if self.config.idle_timeout > Duration::from_secs(0)
                    && idle_conn.idle_time.elapsed() > self.config.idle_timeout
                {
                    // 关闭并丢弃超时连接
                    if let Err(e) = (self.closer)(&idle_conn.conn) {
                        warn!("close timeout conn err: {}", e);
                        // 只记录错误，不中断获取连接流程
                    }
                    let mut count = self.opening_conns.lock().await;
                    *count -= 1;
                    continue;
                }

                // 检查连接生命周期
                if self.config.max_lifetime > Duration::from_secs(0)
                    && idle_conn.created_time.elapsed() > self.config.max_lifetime
                {
                    // 关闭并丢弃超过生命周期的连接
                    if let Err(e) = (self.closer)(&idle_conn.conn) {
                        warn!("close lifetime exceeded conn err: {}", e);
                        // 只记录错误，不中断获取连接流程
                    }
                    let mut count = self.opening_conns.lock().await;
                    *count -= 1;
                    continue;
                }
                
                // 检查连接是否有效
                if let Some(pinger) = &self.pinger {
                    if let Err(_) = pinger(&idle_conn.conn) {
                        if let Err(e) = (self.closer)(&idle_conn.conn) {
                            warn!("关闭无效连接失败: {}", e);
                            // 只记录错误，不中断获取连接流程
                        }
                        let mut count = self.opening_conns.lock().await;
                        *count -= 1;
                        continue;
                    }
                }

                return Ok(PooledConn {
                    conn: idle_conn.conn,
                    created_time: idle_conn.created_time,
                });
            }
        }

        // 检查是否达到最大连接数
        let mut count = self.opening_conns.lock().await;
        if *count >= self.config.max_cap {
            // 创建等待通道
            let (sender, receiver) = oneshot::channel();
            let mut reqs = self.conn_reqs.lock().await;
            reqs.push(ConnRequest { sender });
            drop(reqs);
            drop(count);

            // 等待连接可用
            match receiver.await {
                Ok(result) => result,
                Err(_) => Err(PoolError::MaxActiveConnReached),
            }
        } else {
            // 创建新连接
            match (self.factory)() {
                Ok(conn) => {
                    *count += 1;
                    Ok(PooledConn {
                        conn: Arc::new(conn),
                        created_time: Instant::now(),
                    })
                }
                Err(e) => Err(e),
            }
        }
    }

    /// 归还连接
    pub async fn put(&self, pooled_conn: PooledConn<T>) -> Result<()> {
        let conn = pooled_conn.conn;
        let created_time = pooled_conn.created_time;
        if Arc::strong_count(&conn) == 0 {
            return Err(PoolError::NilConnection);
        }

        if *self.closed.lock().await {
            return Ok(());
        }

        // 首先检查连接生命周期，如果超过最大生命周期，直接关闭
        if self.config.max_lifetime > Duration::from_secs(0)
            && created_time.elapsed() > self.config.max_lifetime
        {
            // 记录并关闭超过生命周期的连接
            warn!("连接生命周期已超过最大限制 ({:?})，直接关闭而非放回连接池", 
                  self.config.max_lifetime);
            
            if let Err(e) = (self.closer)(&conn) {
                warn!("关闭过期连接失败: {}", e);
            }
            let mut count = self.opening_conns.lock().await;
            *count -= 1;
            return Ok(());
        }

        // 检查是否有等待的连接请求
        let mut reqs = self.conn_reqs.lock().await;
        if let Some(req) = reqs.pop() {
            drop(reqs);
            let conn_clone = Arc::clone(&conn);
            if let Err(_) = req.sender.send(Ok(PooledConn {
                conn: conn_clone,
                created_time,
            })) {
                // 如果发送失败，说明接收方已经取消等待，关闭连接
                if let Err(e) = (self.closer)(&conn) {
                    warn!("close timeout conn err: {}", e);
                }
                let mut count = self.opening_conns.lock().await;
                *count -= 1;
            }
            return Ok(());
        }

        let mut conns = self.idle_conns.lock().await;
        if conns.len() < self.config.max_idle {
            // 归还连接到连接池，更新空闲时间，保留创建时间
            conns.push_back(IdleConn {
                conn,
                idle_time: Instant::now(),
                created_time, // 使用原始的创建时间
            });
            Ok(())
        } else {
            // 连接池已满，关闭连接
            if let Err(e) = (self.closer)(&conn) {
                return Err(PoolError::CloseConnection(e));
            }
            let mut count = self.opening_conns.lock().await;
            *count -= 1;
            Ok(())
        }
    }

    /// 关闭连接池
    pub async fn release(&self) -> Result<()> {
        let mut closed = self.closed.lock().await;
        *closed = true;

        // 清空连接池并关闭所有连接
        let mut conns = self.idle_conns.lock().await;
        while let Some(idle_conn) = conns.pop_front() {
            if let Err(e) = (self.closer)(&idle_conn.conn) {
                return Err(PoolError::CloseConnection(e));
            }
        }

        // 关闭所有等待的请求
        let mut reqs = self.conn_reqs.lock().await;
        for req in reqs.drain(..) {
            let _ = req.sender.send(Err(PoolError::Closed));
        }

        // 重置连接计数
        let mut count = self.opening_conns.lock().await;
        *count = 0;
        Ok(())
    }

    /// 获取当前空闲连接数
    pub async fn idle_count(&self) -> usize {
        self.idle_conns.lock().await.len()
    }

    /// 获取当前活跃连接总数
    pub async fn active_count(&self) -> usize {
        *self.opening_conns.lock().await
    }
}
