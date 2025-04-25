use nsq_async_rs::error::Result;
use nsq_async_rs::producer::{new_producer, Producer, ProducerConfig};
use log::{debug, error, info, LevelFilter};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化日志
    env_logger::Builder::new()
        .filter_level(LevelFilter::Debug)
        .format_timestamp_millis()
        .init();

    info!("开始创建生产者...");

    // 创建生产者配置
    let mut config = ProducerConfig::default();
    config.nsqd_addresses = vec!["127.0.0.1:4150".to_string()];
    config.connection_timeout = Duration::from_secs(5);

    debug!("生产者配置: {:?}", config);

    // 创建生产者实例
    let producer = new_producer(config);

    info!("生产者实例已创建，准备发布消息...");

    // 发布单条消息
    let topic = "test_topic";
    let message = "Hello, NSQ!";

    info!("正在发布消息到主题 {}: {}", topic, message);
    match producer.publish(topic, message.as_bytes()).await {
        Ok(_) => info!("消息发布成功"),
        Err(e) => {
            error!("消息发布失败: {}", e);
            return Err(e);
        }
    }

    // 等待确保消息发送完成
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 发布多条消息
    let messages = vec!["消息1", "消息2", "消息3"];

    info!("正在批量发布 {} 条消息到主题 {}", messages.len(), topic);
    match producer.publish_multi(topic, messages.to_vec()).await {
        Ok(_) => info!("批量消息发布成功"),
        Err(e) => {
            error!("批量消息发布失败: {}", e);
            return Err(e);
        }
    }

    // 等待确保消息发送完成
    tokio::time::sleep(Duration::from_millis(100)).await;

    info!("所有消息发布完成");
    Ok(())
}
