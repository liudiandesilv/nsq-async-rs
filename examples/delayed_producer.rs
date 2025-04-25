use nsq_async_rs::producer::{new_producer, ProducerConfig};
use nsq_async_rs::Producer;
use std::error::Error;
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // 创建生产者配置
    let mut config = ProducerConfig::default();

    // 设置NSQ服务器地址
    config.nsqd_addresses = vec!["127.0.0.1:4150".to_string()];

    // 创建NSQ生产者
    let producer = new_producer(config);

    println!("连接到NSQ服务器...");

    // 发送延迟消息
    let topic = "example_delayed_topic";
    let message = "这是一条延迟1秒后发布的消息";
    let delay = Duration::from_millis(1000); // 延迟1秒

    println!("发送延迟消息，延迟时间: {}ms", delay.as_millis());
    let start = Instant::now();

    // 使用延迟发布功能
    let result = producer.publish_delayed(topic, message, delay).await?;

    let elapsed = start.elapsed();
    println!("延迟消息发送成功: {:?}, 耗时: {:?}", result, elapsed);

    // 发送多条不同延迟的消息
    println!("\n发送多条不同延迟的消息...");

    for i in 1..=3 {
        let delay = Duration::from_secs(i); // 1秒, 2秒, 3秒
        let msg = format!("延迟消息 #{} - 延迟{}秒", i, i);

        let result = producer.publish_delayed(topic, msg, delay).await?;
        println!("延迟{}秒的消息已发送: {:?}", i, result);
    }

    println!("示例运行完成！消费者可在接下来的几秒内收到这些消息");

    // 为了演示效果，等待一会儿，这样用户可以观察到延迟消息的效果
    // 注意：在实际应用中，通常不需要等待
    tokio::time::sleep(Duration::from_secs(1)).await;

    Ok(())
}
