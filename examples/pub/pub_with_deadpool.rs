// 标准库导入
use std::time::Duration;

// 第三方库导入
use anyhow::Result;
use deadpool::managed::{Manager, Metrics, Pool, RecycleError, RecycleResult};
use log::{error, info};
use tokio::sync::OnceCell;
use tokio::time::{interval, sleep};

// 本地库导入
use nsq_async_rs::{
    new_producer,
    producer::{NsqProducer, Producer, ProducerConfig},
};

// 定义连接管理器
struct ProducerManager;

impl Manager for ProducerManager {
    type Type = NsqProducer;
    type Error = anyhow::Error;

    async fn create(&self) -> Result<Self::Type, Self::Error> {
        let config = get_producer_config().await;
        let producer = new_producer(config);
        info!("创建新的 NSQ 生产者连接");
        Ok(producer)
    }

    async fn recycle(
        &self,
        producer: &mut Self::Type,
        metrics: &Metrics,
    ) -> RecycleResult<Self::Error> {
        // 检查连接是否健康
        let res = producer.ping(None, Some(Duration::from_millis(500))).await;

        match res {
            Ok(_) => {
                info!(
                    "连接健康检查通过 - 回收次数: {}, 创建时间: {:?}",
                    metrics.recycle_count, metrics.created
                );
                Ok(())
            }
            Err(err) => {
                error!("连接健康检查失败: {}", err);
                Err(RecycleError::Message(
                    format!("连接健康检查失败: {}", err).into(),
                ))
            }
        }
    }
}

// 定义连接池类型
type ProducerPool = Pool<ProducerManager>;

// 使用 OnceCell 存储全局连接池
static PRODUCER_POOL: OnceCell<ProducerPool> = OnceCell::const_new();

// 获取全局连接池实例
async fn get_producer_pool() -> &'static ProducerPool {
    PRODUCER_POOL
        .get_or_init(|| async {
            Pool::builder(ProducerManager)
                .max_size(5) // 最大连接数
                .build()
                .expect("Failed to create producer pool")
        })
        .await
}

// 获取生产者配置
async fn get_producer_config() -> ProducerConfig {
    let mut config = ProducerConfig::default();
    config.connection_timeout = Duration::from_millis(100);
    config.nsqd_addresses = vec!["127.0.0.1:4150".to_string()];
    config.nsqlookupd_addresses = vec!["127.0.0.1:4161".to_string()];
    config
}

// 使用连接池发送消息的示例函数
async fn send_message(topic: &str, message: &str) -> Result<()> {
    let pool = get_producer_pool().await;
    let producer = pool.get().await.map_err(|e| anyhow::anyhow!("{}", e))?;

    producer
        .publish(topic, message.as_bytes())
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    Ok(())
}

// 打印连接池状态
async fn print_pool_status() {
    let pool = get_producer_pool().await;
    let status = pool.status();
    info!(
        "连接池状态 - 大小: {}, 可用: {}, 等待: {}",
        status.size, status.available, status.waiting
    );
}

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化日志
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Info)
        .format_timestamp_millis()
        .init();

    let topic = "test_topic";
    let mut interval = interval(Duration::from_secs(5));
    let mut message_counter = 0;

    info!("开始定时发送消息测试...");

    loop {
        interval.tick().await;

        // 打印连接池状态
        print_pool_status().await;

        // 发送消息

        let mut tasks = Vec::new();
        for _ in 0..10 {
            message_counter += 1;
            let message = format!("定时消息 #{}", message_counter);
            let handle = tokio::spawn(async move {
                match send_message(topic, &message).await {
                    Ok(_) => info!("成功发送消息: {}", message),
                    Err(e) => error!("发送消息失败: {}", e),
                }
            });
            tasks.push(handle);
        }
        for task in tasks {
            task.await.unwrap();
        }

        // 每发送10条消息后暂停一下，观察连接池行为
        if message_counter % 10 == 0 {
            info!("暂停5秒观察连接池行为...");
            sleep(Duration::from_secs(5)).await;
        }
    }
}
