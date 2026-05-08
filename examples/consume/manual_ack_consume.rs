use async_trait::async_trait;
use log::{LevelFilter, error, info, warn};
use nsq_async_rs::consumer::{Consumer, ConsumerConfig, Handler};
use nsq_async_rs::error::Result;
use nsq_async_rs::protocol::Message;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc};

/// 手动确认的并发消息处理器
///
/// 这个示例展示了如何使用手动确认来处理并发消息
/// 消息会被发送到 worker 线程池异步处理，处理完成后手动确认
struct ManualAckHandler {
    /// 工作线程数量
    worker_count: usize,
    /// 消息发送通道
    sender: Arc<Mutex<mpsc::Sender<Message>>>,
}

impl ManualAckHandler {
    /// 创建新的手动确认处理器
    pub fn new(worker_count: usize) -> Self {
        // 创建消息通道，缓冲区大小为工作线程数量的 10 倍
        let (tx, rx) = mpsc::channel(worker_count * 10);
        let sender = Arc::new(Mutex::new(tx));
        let receiver = Arc::new(Mutex::new(rx));

        // 创建处理器
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
                    let msg_id = msg.id_string();

                    // 随机休眠 10ms - 500ms 模拟处理时间
                    let sleep_time = rand::random::<u64>() % 490 + 10;
                    tokio::time::sleep(Duration::from_millis(sleep_time)).await;

                    // 模拟处理逻辑：30% 的概率失败
                    let should_fail = rand::random::<f32>() < 0.3;

                    if should_fail {
                        warn!(
                            "工作线程 {} - 消息处理失败，重新入队: ID={}, 尝试次数: {}",
                            worker_id, msg_id, msg.attempts
                        );

                        // 如果尝试次数超过 3 次，就不再重试
                        if msg.attempts > 3 {
                            warn!(
                                "工作线程 {} - 消息重试次数过多，丢弃: ID={}",
                                worker_id, msg_id
                            );
                            // 手动 FIN，表示不再处理
                            if let Err(e) = msg.finish().await {
                                error!("工作线程 {} - 发送 FIN 失败: {}", worker_id, e);
                            }
                        } else {
                            // 手动 REQ，延迟 5 秒后重试
                            if let Err(e) = msg.requeue(5000).await {
                                error!("工作线程 {} - 发送 REQ 失败: {}", worker_id, e);
                            }
                        }
                    } else {
                        info!(
                            "工作线程 {} 成功处理消息 (耗时 {}ms) - ID: {}, 尝试次数: {}, 内容: {}",
                            worker_id,
                            sleep_time,
                            msg_id,
                            msg.attempts,
                            String::from_utf8_lossy(&msg.body)
                        );

                        // 手动 FIN
                        if let Err(e) = msg.finish().await {
                            error!("工作线程 {} - 发送 FIN 失败: {}", worker_id, e);
                        }
                    }
                }
            });
        }

        info!("已启动 {} 个工作线程处理消息", self.worker_count);
    }
}

#[async_trait]
impl Handler for ManualAckHandler {
    async fn handle_message(&self, message: Message) -> Result<()> {
        // 记录消息接收
        let msg_id = message.id_string();

        // 将消息发送到通道，由工作线程异步处理
        let sender = self.sender.lock().await;

        // 先尝试非阻塞方式发送
        let send_result = sender.try_send(message.clone());
        match send_result {
            Ok(_) => {
                info!("消息已发送到工作线程通道: ID={}", msg_id);
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
        // 注意：由于启用了手动确认，这里返回 Ok 不会自动发送 FIN
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

    info!("正在初始化手动确认的 NSQ 消费者...");

    // 创建消费者配置
    let config = ConsumerConfig {
        max_in_flight: 100,          // 增加同时处理的最大消息数
        max_attempts: 5,             // 最大重试次数
        disable_auto_response: true, // 【关键】禁用自动响应，启用手动确认
        concurrent_handlers: 20,     // 与 worker_count 对齐
        ..Default::default()
    };

    // 创建手动确认的并发消息处理器，指定 20 个工作线程
    let handler = ManualAckHandler::new(20);

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
    info!("使用手动确认模式，消息会在 worker 线程中异步处理和确认");
    info!("按 Ctrl+C 停止消费者...");

    // 等待中断信号
    tokio::signal::ctrl_c().await?;

    info!("收到停止信号，正在优雅关闭...");

    // 停止消费者
    consumer.stop().await?;

    info!("消费者已关闭");
    Ok(())
}
