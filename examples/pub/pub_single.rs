use log::info;
use nsq_async_rs::{
    connection_pool::{create_connection_pool, ConnectionPoolConfig},
    producer::new_producer,
    Producer, ProducerConfig,
};
use std::error::Error;
use std::sync::Arc;
use std::time::Instant;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // 配置日志
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Info)
        .format_timestamp_millis()
        .init();

    info!("start pub single msg...");

    let pool_config = ConnectionPoolConfig {
        max_connections_per_host: 20,
        ..Default::default()
    };
    let pool = create_connection_pool(pool_config);

    let producer = Arc::new(new_producer(ProducerConfig::default()).with_connection_pool(pool));

    // 预热连接池

    let mut handlers = vec![];

    // 创建10个并发任务，共享Producer实例
    for i in 1..=10 {
        let producer_clone = producer.clone();
        let handler = tokio::spawn(async move {
            let start = Instant::now();

            // 发送单条消息
            let topic = "test_topic"; 
            let message = format!("async msg #{}", i);

            // 使用trait方法
            let result = producer_clone.publish(topic, message).await;
            let success = result.is_ok();

            let elapsed = start.elapsed();
            info!("#{} pub single msg: {}, use: {:?}", i, success, elapsed);
        });
        handlers.push(handler);
    }

    // 等待所有异步任务完成
    for handler in handlers {
        handler.await?;
    }

    // 2. 同步测试 - 也使用共享的Producer实例
    info!("pub single msg...");

    // 使用同步方式测试发送消息
    // 直接复用已初始化的全局生产者
    let producer = producer.clone();

    // 预热
    let topic = "test_topic";
    // 使用trait方法
    let _ = producer.publish(topic, "warmup").await;

    for i in 1..=10 {
        let start = Instant::now();

        // 发送单条消息
        let topic = "test_topic";
        let message = format!("同步消息 #{}", i);

        // 使用trait方法
        let result = producer.publish(topic, message).await;
        let success = result.is_ok();

        let elapsed = start.elapsed();
        info!("#{} pub single msg: {}, use: {:?}", i, success, elapsed);
    }

    info!("pub delay msg...");

    let topic = "test_topic";

    for i in 1..=10 {
        let start = Instant::now();
        let message = format!("延迟消息 #{}", i);
        let result = producer
            .publish_delayed(topic, message, std::time::Duration::from_secs(10))
            .await;
        let success = result.is_ok();
        info!(
            "#{} pub delay msg: {}, use: {:?}",
            i,
            success,
            start.elapsed()
        );
    }

    // 暂停一下，让日志输出完成
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    info!("pub single msg done");
    Ok(())
}

#[cfg(test)]
mod pub_test {
    use super::*;
    use chrono::Local;
    use log::info;
    use nsq_async_rs::Producer;
    use std::time::Instant;

    /// test nsq-async-rs with connection pool
    #[tokio::test]
    async fn test_interval_pub() {
        // 初始化日志
        env_logger::Builder::new()
            .filter_level(log::LevelFilter::Info)
            .format_timestamp_millis()
            .init();

        info!("开始运行定时发送测试");

        // 使用定时器发送消息
        let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(500));
        let topic = "test_interval";

        let pool_config = ConnectionPoolConfig {
            max_connections_per_host: 20,
            ..Default::default()
        };
        let pool = create_connection_pool(pool_config);

        let producer = Arc::new(new_producer(ProducerConfig::default()).with_connection_pool(pool));

        let mut i = 1;
        loop {
            interval.tick().await;
            let producer = producer.clone();
            let start = Instant::now();
            let now = Local::now();

            // 多异步
            let mut handlers = vec![];
            for j in 1..=10 {
                let producer_clone = producer.clone();
                let handler = tokio::spawn(async move {
                    let message = format!("定时消息 #{}", now.format("%Y-%m-%d %H:%M:%S.%3f"));

                    let result = producer_clone.publish(topic, message).await;
                    let success = result.is_ok();
                    info!(
                        "第{}批次,#{} 发送定时消息: {}, use: {:?}",
                        i,
                        j,
                        success,
                        start.elapsed()
                    );
                });
                handlers.push(handler);
            }

            for handler in handlers {
                let _ = handler.await;
            }

            info!("第{}批次 发送定时消息完成", i);

            i += 1;
        }
    }

    /// test nsq-async-rs without connection pool
    #[tokio::test]
    async fn test_intervalpub_without_pool() {
        // 初始化日志
        env_logger::Builder::new()
            .filter_level(log::LevelFilter::Info)
            .format_timestamp_millis()
            .init();

        info!("开始运行定时发送测试");

        // 使用定时器发送消息
        let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(500));
        let topic = "test_interval";

        let mut i = 1;
        loop {
            interval.tick().await;
            let start = Instant::now();
            let now = Local::now();

            let mut config = ProducerConfig::default();
            config.nsqd_addresses = vec!["127.0.0.1:4150".to_string()];
            config.connection_timeout = tokio::time::Duration::from_secs(5);
            // 多异步
            let mut handlers = vec![];

            for j in 1..=10 {
                let producer = new_producer(config.clone());
                let handler = tokio::spawn(async move {
                    let message = format!("定时消息 #{}", now.format("%Y-%m-%d %H:%M:%S.%3f"));
                    let result = producer.publish(topic, message).await;
                    let success = result.is_ok();
                    info!(
                        "第{}批次,#{} 发送定时消息: {}, use: {:?}",
                        i,
                        j,
                        success,
                        start.elapsed()
                    );
                });
                handlers.push(handler);
            }

            for handler in handlers {
                let _ = handler.await;
            }

            info!("第{}批次 发送定时消息完成", i);

            i += 1;
        }
    }
}
