use async_trait::async_trait;
use log::{error, info, warn, LevelFilter};
use nsq_async_rs::consumer::{Consumer, ConsumerConfig, Handler};
use nsq_async_rs::error::Result;
use nsq_async_rs::protocol::Message;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};

/// 并发消息处理器
struct ConcurrentMessageHandler {
    /// 工作线程数量
    worker_count: usize,
    /// 消息发送通道
    sender: Arc<Mutex<mpsc::Sender<Message>>>,
}

impl ConcurrentMessageHandler {
    /// 创建新的并发消息处理器
    pub fn new(worker_count: usize) -> Self {
        // 创建消息通道，缓冲区大小为工作线程数量的 10 倍
        let (tx, rx) = mpsc::channel(worker_count * 10);
        let sender = Arc::new(Mutex::new(tx));
        let receiver = Arc::new(Mutex::new(rx));

        // 创建并发处理器
        let handler = Self {
            worker_count,
            sender,
        };

        // 启动工作线程
        handler.start_workers(receiver);

        handler
    }

    /// 启动工作线程
    fn start_workers(&self, receiver: Arc<Mutex<mpsc::Receiver<Message>>>) {
        for i in 0..self.worker_count {
            let worker_id = i + 1;
            let rx = receiver.clone();

            // 启动工作线程
            tokio::spawn(async move {
                info!("工作线程 {} 已启动", worker_id);

                loop {
                    // 从通道获取消息
                    let msg = {
                        let mut rx_guard = rx.lock().await;
                        match rx_guard.recv().await {
                            Some(msg) => msg,
                            None => {
                                info!("工作线程 {} 的消息通道已关闭，退出处理循环", worker_id);
                                break;
                            }
                        }
                    };

                    // 处理消息
                    let msg_id = String::from_utf8_lossy(&msg.id).to_string();
                    // info!("工作线程 {} 正在处理消息: ID={}", worker_id, msg_id);

                    //随机休眠 10ms - 500ms
                    let sleep_time = rand::random::<u64>() % 400 + 10;
                    tokio::time::sleep(Duration::from_millis(sleep_time)).await;

                    // 实际的消息处理逻辑
                    if msg.attempts > 3 {
                        warn!(
                            "工作线程 {} - 消息重试次数过多，将被丢弃: ID={}",
                            worker_id, msg_id
                        );
                        continue;
                    }

                    info!(
                        "工作线程 {} 使用 {}ms 处理消息 - ID: {}, 尝试次数: {}, 内容: {}",
                        worker_id,
                        sleep_time,
                        msg_id,
                        msg.attempts,
                        String::from_utf8_lossy(&msg.body)
                    );
                }
            });
        }

        info!("已启动 {} 个工作线程处理消息", self.worker_count);
    }
}

#[async_trait]
impl Handler for ConcurrentMessageHandler {
    async fn handle_message(&self, message: Message) -> Result<()> {
        // 记录消息接收
        let msg_id = String::from_utf8_lossy(&message.id).to_string();

        // 将消息发送到通道，由工作线程处理
        let sender = self.sender.lock().await;

        // 先尝试非阻塞方式发送
        let send_result = sender.try_send(message.clone());
        match send_result {
            Ok(_) => {
                info!("消息已快速发送到工作线程通道: ID={}", msg_id);
            }
            Err(mpsc::error::TrySendError::Full(msg)) => {
                // 通道满了，使用阻塞方式发送
                info!("通道满了，尝试阻塞发送消息: ID={}", msg_id);
                if let Err(e) = sender.send(msg).await {
                    error!("发送消息到工作线程通道失败: {}", e);
                    return Err(nsq_async_rs::error::Error::Other(format!(
                        "发送消息到工作线程通道失败: {}",
                        e
                    )));
                }
                info!("消息已阻塞发送到工作线程通道: ID={}", msg_id);
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                // 通道已断开
                error!("工作线程通道已断开: ID={}", msg_id);
                return Err(nsq_async_rs::error::Error::Other(format!(
                    "工作线程通道已断开: ID={}",
                    msg_id
                )));
            }
        }

        // 消息已发送到工作线程通道，返回成功
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
        max_in_flight: 100, // 增加同时处理的最大消息数，确保有足够的消息可供并发处理
        max_attempts: 5,    // 最大重试次数
        dial_timeout: Duration::from_secs(1),
        read_timeout: Duration::from_secs(60),
        write_timeout: Duration::from_secs(1),
        lookup_poll_interval: Duration::from_secs(60),
        lookup_poll_jitter: 0.3,
        max_requeue_delay: Duration::from_secs(15 * 60),
        default_requeue_delay: Duration::from_secs(90),
        shutdown_timeout: Duration::from_secs(30),
        backoff_strategy: true, // 启用指数退避重连策略
    };

    // 创建并发消息处理器，指定 20 个工作线程
    let handler = ConcurrentMessageHandler::new(20);

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
