use chrono::Local;
use nsq_async_rs::producer::{new_producer, ProducerConfig};
use nsq_async_rs::Producer;
use std::error::Error;
use std::time::Instant;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // 创建生产者配置
    let mut config = ProducerConfig::default();
    config.nsqd_addresses = vec!["127.0.0.1:4150".to_string()];

    // 创建NSQ生产者
    let producer = new_producer(config);

    println!("连接到NSQ服务器...");

    // 准备发送的多条消息
    let topic = "test_topic";
    let mut messages = vec![];

    for i in 0..100 {
        messages.push(format!(
            "第一条消息 #{},at:{}",
            i + 1,
            Local::now().to_string()
        ));
    }

    println!("准备向主题 '{}' 发送 {} 条消息...", topic, messages.len());

    // 1. 使用传统方式逐条发送
    let start_individual = Instant::now();

    for (i, msg) in messages.iter().enumerate() {
        producer.publish(topic, msg).await?;
        println!("单条发送: 消息 #{} 已发送", i + 1);
    }

    let individual_elapsed = start_individual.elapsed();
    println!(
        "逐条发送 {} 条消息耗时: {:?}",
        messages.len(),
        individual_elapsed
    );
    println!(
        "平均每条消息耗时: {:?}",
        individual_elapsed / messages.len() as u32
    );

    let mut multi_messages = vec![];

    for i in 0..100 {
        // 创建消息字符串
        let message = format!("多条消息 #{},at:{}", i + 1, Local::now().to_string());

        // 将消息转换为字节数组
        multi_messages.push(message);
    }

    let start_multi = Instant::now();
    // 直接发送字节数组
    producer
        .publish_multi(topic, multi_messages.to_vec())
        .await?;
    let multi_elapsed = start_multi.elapsed();

    println!(
        "\n使用publish_multi发送字节数组，{} 条消息耗时: {:?}",
        multi_messages.len(),
        multi_elapsed
    );
    println!(
        "平均每条消息耗时: {:?}",
        multi_elapsed / multi_messages.len() as u32
    );

    // 计算性能提升
    let speedup = individual_elapsed.as_micros() as f64 / multi_elapsed.as_micros() as f64;
    println!("使用字节数组的性能提升: {:.2}倍", speedup);

    Ok(())
}
