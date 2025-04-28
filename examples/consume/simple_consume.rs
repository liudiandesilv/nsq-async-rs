use async_trait::async_trait;
use log::{info, warn, LevelFilter};
use nsq_async_rs::consumer::{Consumer, ConsumerConfig, Handler};
use nsq_async_rs::error::Result;
use nsq_async_rs::protocol::Message;
use std::time::Duration;

/// 简单消息处理器
#[derive(Default)]
struct SimpleMessageHandler;

#[async_trait]
impl Handler for SimpleMessageHandler {
    async fn handle_message(&self, message: Message) -> Result<()> {
        // 获取消息ID和内容
        let msg_id = String::from_utf8_lossy(&message.id).to_string();
        let content = String::from_utf8_lossy(&message.body).to_string();

        info!(
            "收到消息 - ID: {}, 尝试次数: {}, 内容: {}",
            msg_id, message.attempts, content
        );

        // 模拟消息处理
        // 如果消息尝试次数过多，可以选择不处理
        if message.attempts > 3 {
            warn!("消息重试次数过多，将被丢弃: ID={}", msg_id);
            return Ok(());
        }

        // 随机休眠一段时间，模拟处理时间
        let sleep_time = rand::random::<u64>() % 400 + 10;
        tokio::time::sleep(Duration::from_millis(sleep_time)).await;

        info!("消息处理完成 - ID: {}, 处理耗时: {}ms", msg_id, sleep_time);

        // 返回成功表示消息已成功处理
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // 配置日志
    env_logger::Builder::new()
        .filter_level(LevelFilter::Info)
        .format_timestamp_millis()
        .init();

    info!("正在初始化简单 NSQ 消费者...");

    // 创建消费者配置
    let config = ConsumerConfig {
        max_in_flight: 1, // 一次只处理一条消息
        max_attempts: 5,  // 最大重试次数
        ..Default::default()
    };

    // 创建简单消息处理器
    let handler = SimpleMessageHandler::default();

    // 创建消费者实例
    let consumer = Consumer::new(
        "test_topic".to_string(),   // 主题名称
        "test_channel".to_string(), // 频道名称
        config,
        handler,
    )?;

    info!("消费者已创建，正在连接到 NSQ...");

    // 连接到 nsqlookupd
    consumer
        .connect_to_nsqlookupd("http://127.0.0.1:4161".to_string())
        .await?;
    info!("已连接到 nsqlookupd");

    // 启动消费者
    consumer.start().await?;

    info!("消费者已启动，正在监听主题: test_topic");
    info!("按 Ctrl+C 停止消费者...");

    // 等待中断信号
    tokio::signal::ctrl_c().await?;

    info!("收到停止信号，正在优雅关闭...");

    // 停止消费者
    consumer.stop().await?;

    info!("消费者已关闭");
    Ok(())
}
