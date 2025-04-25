use async_trait::async_trait;
use log::{info, warn, LevelFilter};
use nsq_async_rs::consumer::{Consumer, ConsumerConfig, Handler};
use nsq_async_rs::error::Result;
use nsq_async_rs::protocol::Message;
use std::time::Duration;

/// 消息处理器
#[derive(Default)]
struct MessageHandler;

#[async_trait]
impl Handler for MessageHandler {
    async fn handle_message(&self, message: Message) -> Result<()> {
        info!(
            "收到消息 - ID: {:?}, 尝试次数: {}, 内容: {:?}",
            String::from_utf8_lossy(&message.id),
            message.attempts,
            String::from_utf8_lossy(&message.body)
        );

        // 这里可以添加你的业务逻辑
        if message.attempts > 3 {
            warn!("消息重试次数过多，将被丢弃");
            return Ok(());
        }

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

    info!("正在初始化 NSQ 消费者...");

    // 创建消费者配置
    let config = ConsumerConfig {
        max_in_flight: 1, // 同时处理的最大消息数
        max_attempts: 5,  // 最大重试次数
        dial_timeout: Duration::from_secs(1),
        read_timeout: Duration::from_secs(60),
        write_timeout: Duration::from_secs(1),
        lookup_poll_interval: Duration::from_secs(60),
        lookup_poll_jitter: 0.3,
        max_requeue_delay: Duration::from_secs(15 * 60),
        default_requeue_delay: Duration::from_secs(90),
        shutdown_timeout: Duration::from_secs(30), // 添加关闭超时时间
    };

    // 创建消费者实例
    let consumer = Consumer::new(
        "test_topic".to_string(),   // 主题名称
        "test_channel".to_string(), // 频道名称
        config,
        MessageHandler::default(),
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
