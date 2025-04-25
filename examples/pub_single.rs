use nsq_async_rs::connection_pool::create_default_connection_pool;
use nsq_async_rs::producer::{new_producer, NsqProducer};
use nsq_async_rs::Producer; // 添加Producer trait导入
use nsq_async_rs::ProducerConfig;
use log::info;
use std::error::Error;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::OnceCell as AsyncOnceCell;

// 只使用一个全局生产者静态变量
static GLOBAL_PRODUCER: AsyncOnceCell<Arc<NsqProducer>> = AsyncOnceCell::const_new();

// 异步获取全局生产者
async fn get_producer() -> Arc<NsqProducer> {
    GLOBAL_PRODUCER
        .get_or_init(|| async {
            let mut config = ProducerConfig::default();
            config.nsqd_addresses = vec!["127.0.0.1:4150".to_string()];

            // 创建连接池并直接与Producer关联
            info!("应用层初始化全局连接池和生产者");
            let pool = create_default_connection_pool();
            let producer = new_producer(config).with_connection_pool(pool);

            // 预热连接池
            let topic = "test_topic";
            // 确保使用trait方法
            let _ = Producer::publish(&producer, topic, "warmup").await;

            info!("全局生产者初始化完成");
            Arc::new(producer)
        })
        .await
        .clone()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // 配置日志
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Info)
        .format_timestamp_millis()
        .init();

    info!("开始 NSQ 性能测试 (使用OnceCell全局连接池)...");

    // 1. 异步测试 - 使用全局生产者
    let producer = get_producer().await;

    // 预热连接池
    let topic = "test_topic";
    let _ = producer.publish(topic, "warmup").await;

    let mut handlers = vec![];

    // 创建10个并发任务，共享Producer实例
    for i in 1..=10 {
        let producer_clone = producer.clone();
        let handler = tokio::spawn(async move {
            let start = Instant::now();

            // 发送单条消息
            let topic = "test_topic";
            let message = format!("异步消息 #{}", i);

            // 使用trait方法
            let result = Producer::publish(&*producer_clone, topic, message).await;
            let success = result.is_ok();

            let elapsed = start.elapsed();
            info!("#{} 异步消息发送成功: {}, 耗时: {:?}", i, success, elapsed);
        });
        handlers.push(handler);
    }

    // 等待所有异步任务完成
    for handler in handlers {
        handler.await?;
    }

    // 2. 同步测试 - 也使用共享的Producer实例
    info!("开始同步发布消息...");

    // 使用同步方式测试发送消息
    // 直接复用已初始化的全局生产者
    let producer = get_producer().await;

    // 预热
    let topic = "test_topic";
    // 使用trait方法
    let _ = Producer::publish(&*producer, topic, "warmup").await;

    for i in 1..=10 {
        let start = Instant::now();

        // 发送单条消息
        let topic = "test_topic";
        let message = format!("同步消息 #{}", i);

        // 使用trait方法
        let result = Producer::publish(&*producer, topic, message).await;
        let success = result.is_ok();

        let elapsed = start.elapsed();
        info!("#{} 同步消息发送成功: {}, 耗时: {:?}", i, success, elapsed);
    }

    // 暂停一下，让日志输出完成
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    info!("NSQ 性能测试完成");
    Ok(())
}
