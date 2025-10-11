# nsq-async-rs

[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Crates.io](https://img.shields.io/crates/v/nsq-async-rs.svg)](https://crates.io/crates/nsq-async-rs)

*其他语言版本: [English](README.md), [简体中文](README_zh.md)*

nsq-async-rs 是一个用 Rust 编写的高性能、可靠的 NSQ 客户端库。该项目参考了官方的 [go-nsq](https://github.com/nsqio/go-nsq) 实现，并在 Rust 生态系统中提供了类似的功能和接口。

## 特性

- ✨ 异步 I/O 支持（基于 tokio）
- 🚀 高性能消息处理
- 🔄 自动重连和错误重试
- 🔍 支持 nsqlookupd 服务发现
- 🛡️ 优雅关闭支持
- 📊 内置消息统计
- ⚡ 支持延迟发布
- 📦 支持批量发布
- 🔀 支持并发消息处理
- 🏊‍♂️ 内置生产者连接池
- 🎯 **支持手动消息确认（Manual Acknowledgement）**
- 💫 与官方 go-nsq 保持一致的功能特性

## 安装

将以下依赖添加到你的 `Cargo.toml` 文件中：

```toml
[dependencies]
nsq-async-rs = "0.1.8"
```

## 快速开始

### 基本消费者示例

```rust
use nsq_async_rs::consumer::{Consumer, ConsumerConfig, Handler};
use nsq_async_rs::error::Result;
use nsq_async_rs::protocol::Message;

#[derive(Default)]
struct MessageHandler;

#[async_trait::async_trait]
impl Handler for MessageHandler {
    async fn handle_message(&self, message: Message) -> Result<()> {
        println!("收到消息: {:?}", String::from_utf8_lossy(&message.body));
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = ConsumerConfig::default();
    let consumer = Consumer::new(
        "test_topic".to_string(),
        "test_channel".to_string(),
        config,
        MessageHandler::default(),
    )?;

    consumer.connect_to_nsqlookupd("http://127.0.0.1:4161".to_string()).await?;
    consumer.start().await?;

    tokio::signal::ctrl_c().await?;
    consumer.stop().await?;
    Ok(())
}
```

### 并发消费者示例

```rust
use async_trait::async_trait;
use log::{error, info};
use nsq_async_rs::consumer::{Consumer, ConsumerConfig, Handler};
use nsq_async_rs::error::Result;
use nsq_async_rs::protocol::Message;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};

/// 并发消息处理器
struct ConcurrentMessageHandler {
    worker_count: usize,
    sender: Arc<Mutex<mpsc::Sender<Message>>>,
}

impl ConcurrentMessageHandler {
    pub fn new(worker_count: usize) -> Self {
        // 创建消息通道，缓冲区大小为工作线程数量的 10 倍
        let (tx, rx) = mpsc::channel(worker_count * 10);
        let sender = Arc::new(Mutex::new(tx));
        let receiver = Arc::new(Mutex::new(rx));

        let handler = Self {
            worker_count,
            sender,
        };

        // 启动工作线程
        handler.start_workers(receiver);

        handler
    }

    fn start_workers(&self, receiver: Arc<Mutex<mpsc::Receiver<Message>>>) {
        for i in 0..self.worker_count {
            let worker_id = i + 1;
            let rx = receiver.clone();

            tokio::spawn(async move {
                info!("工作线程 {} 已启动", worker_id);

                loop {
                    // 从通道获取消息
                    let msg = {
                        let mut rx_guard = rx.lock().await;
                        match rx_guard.recv().await {
                            Some(msg) => msg,
                            None => break,
                        }
                    };

                    // 处理消息
                    let msg_id = String::from_utf8_lossy(&msg.id).to_string();
                    info!("工作线程 {} 正在处理消息: {}", worker_id, msg_id);
                    
                    // 在这里添加你的消息处理逻辑
                    
                    info!("工作线程 {} 完成消息处理: {}", worker_id, msg_id);
                }
            });
        }
    }
}

#[async_trait]
impl Handler for ConcurrentMessageHandler {
    async fn handle_message(&self, message: Message) -> Result<()> {
        let msg_id = String::from_utf8_lossy(&message.id).to_string();
        let sender = self.sender.lock().await;

        // 先尝试非阻塞方式发送
        let send_result = sender.try_send(message.clone());
        match send_result {
            Ok(_) => {
                info!("消息已发送到工作线程通道: ID={}", msg_id);
            }
            Err(mpsc::error::TrySendError::Full(msg)) => {
                // 通道满了，使用阻塞方式发送
                if let Err(e) = sender.send(msg).await {
                    error!("发送消息到工作线程通道失败: {}", e);
                    return Err(nsq_async_rs::error::Error::Other(e.to_string()));
                }
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                error!("工作线程通道已断开: ID={}", msg_id);
                return Err(nsq_async_rs::error::Error::Other("工作线程通道已断开".into()));
            }
        }

        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // 创建消费者配置
    let config = ConsumerConfig {
        max_in_flight: 100, // 增加以提高吞吐量
        max_attempts: 5,
        // 其他配置选项...
        ..Default::default()
    };

    // 创建并发处理器，指定 20 个工作线程
    let handler = ConcurrentMessageHandler::new(20);

    // 创建消费者
    let consumer = Consumer::new(
        "test_topic".to_string(),
        "test_channel".to_string(),
        config,
        handler,
    )?;

    consumer.connect_to_nsqlookupd("http://127.0.0.1:4161".to_string()).await?;
    consumer.start().await?;

    tokio::signal::ctrl_c().await?;
    consumer.stop().await?;
    Ok(())
}
```

### 基本生产者示例

```rust
use nsq_async_rs::producer::Producer;
use nsq_async_rs::error::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let producer = Producer::connect("127.0.0.1:4150").await?;
    
    producer.publish("test_topic", "Hello, NSQ!".as_bytes()).await?;
    Ok(())
}
```

### 使用连接池的生产者示例

```rust
use log::{error, info};
use nsq_async_rs::{
    producer::{new_producer, NsqProducer},
    Producer, ProducerConfig,
};
use std::error::Error;
use std::time::Duration;
use nsq_async_rs::pool::{Pool, PoolConfig, PoolError};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // 配置连接池
    let pool_config = PoolConfig {
        initial_cap: 3,            // 初始连接数
        max_cap: 10,               // 最大连接数
        max_idle: 5,               // 最大空闲连接
        idle_timeout: Duration::from_secs(30),  // 空闲超时时间
        max_lifetime: Duration::from_secs(300), // 连接最大生命周期（5分钟）
    };

    // 创建 NSQ 生产者连接池
    let pool = Pool::new(
        pool_config,
        // 创建连接的工厂函数
        || {
            let p_cfg = ProducerConfig {
                nsqd_addresses: vec!["127.0.0.1:4150".to_string()],
                ..Default::default()
            };
            Ok(new_producer(p_cfg))
        },
        // 关闭连接的函数（NSQ 生产者不需要显式关闭）
        |_producer| Ok(()),
        // 可选的连接检查函数
        Some(|_producer| Ok(())),
    ).await?;

    // 从连接池获取一个连接
    let topic = "test_topic";
    let pooled_conn = pool.get().await?;
    
    // 使用连接
    match pooled_conn.conn.publish(topic, "来自连接池的消息！").await {
        Ok(_) => info!("消息发布成功"),
        Err(e) => error!("消息发布失败: {}", e),
    }
    
    // 归还连接到连接池
    pool.put(pooled_conn).await?;
    
    // 使用完毕后关闭连接池
    pool.release().await?;
    
    Ok(())
}
```

### 批量发布示例

```rust
use chrono::Local;
use nsq_async_rs::producer::{new_producer, ProducerConfig};
use std::error::Error;
use std::time::Instant;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // 创建生产者配置
    let mut config = ProducerConfig::default();
    config.nsqd_addresses = vec!["127.0.0.1:4150".to_string()];

    // 创建 NSQ 生产者
    let producer = new_producer(config);
    let topic = "test_topic";
    
    // 准备多条消息
    let mut messages = vec![];
    for i in 0..100 {
        messages.push(format!(
            "消息 #{},时间:{}",
            i + 1,
            Local::now().to_string()
        ));
    }

    // 测量批量发布性能
    let start = Instant::now();
    producer.publish_multi(topic, messages).await?;
    let elapsed = start.elapsed();

    println!("批量发布 100 条消息耗时: {:?}", elapsed);
    println!("平均每条消息耗时: {:?}", elapsed / 100);

    Ok(())
}
```

## 配置选项

### 消费者配置

```rust
ConsumerConfig {
    max_in_flight: 100,                  // 同时处理的最大消息数
    max_attempts: 5,                     // 最大重试次数
    dial_timeout: Duration::from_secs(1),  // 连接超时
    read_timeout: Duration::from_secs(60), // 读取超时
    write_timeout: Duration::from_secs(1), // 写入超时
    lookup_poll_interval: Duration::from_secs(60), // nsqlookupd 轮询间隔
    lookup_poll_jitter: 0.3,              // 轮询抖动系数
    max_requeue_delay: Duration::from_secs(15 * 60), // 最大重新入队延迟
    default_requeue_delay: Duration::from_secs(90),  // 默认重新入队延迟
    shutdown_timeout: Duration::from_secs(30),       // 关闭超时
    backoff_strategy: true,                // 启用指数退避重连策略
}
```

### 连接池配置

```rust
PoolConfig {
    initial_cap: 5,                    // 初始连接数
    max_cap: 20,                       // 最大连接数
    max_idle: 10,                      // 最大空闲连接数
    idle_timeout: Duration::from_secs(30), // 连接空闲超时时间
    max_lifetime: Duration::from_secs(300), // 连接最大生命周期（5分钟）
}
```

## 高级功能

### 连接健康检查 (Ping)

```rust
// 使用默认超时时间（5秒）进行 ping
let result = producer.ping(None, None).await;

// 使用自定义地址和超时时间进行 ping
let result = producer.ping(
    Some("127.0.0.1:4150"),
    Some(Duration::from_millis(500))
).await;

// 在继续操作前检查 ping 结果
if let Err(err) = result {
    println!("NSQ服务器连接异常: {}", err);
    // 处理连接异常...
}
```

### 延迟发布

```rust
producer.publish_with_delay("test_topic", "延迟消息".as_bytes(), Duration::from_secs(60)).await?;
```

### 批量发布

```rust
let messages = vec![
    "消息1".as_bytes().to_vec(),
    "消息2".as_bytes().to_vec(),
    "消息3".as_bytes().to_vec(),
];
producer.publish_multiple("test_topic", messages).await?;
```

## 手动消息确认

nsq-async-rs 支持手动消息确认，允许你完全控制消息的确认时机。这对于并发处理、批量处理或复杂的错误处理场景非常有用。

### 启用手动确认

```rust
let config = ConsumerConfig {
    max_in_flight: 100,
    disable_auto_response: true, // 启用手动确认
    ..Default::default()
};
```

### 手动确认消息

```rust
#[async_trait]
impl Handler for MyHandler {
    async fn handle_message(&self, message: Message) -> Result<()> {
        // 发送到工作线程异步处理
        self.worker_tx.send(message).await?;
        
        // 返回 Ok，但不会自动 FIN（因为启用了手动确认）
        Ok(())
    }
}

// 在工作线程中
async fn worker(message: Message) {
    match process_message(&message.body).await {
        Ok(_) => {
            // 手动发送 FIN
            message.finish().await.unwrap();
        }
        Err(_) => {
            // 手动重新入队，延迟 5 秒
            message.requeue(5000).await.unwrap();
        }
    }
}
```

### API 方法

- `message.finish()` - 发送 FIN 命令，标记消息处理成功
- `message.requeue(delay)` - 发送 REQ 命令，重新入队消息
- `message.touch()` - 发送 TOUCH 命令，重置消息超时
- `message.disable_auto_response()` - 禁用单条消息的自动响应

### 运行示例

```bash
# 运行手动确认示例
cargo run --example manual_ack_consume

# 运行自动确认示例（对比）
cargo run --example simple_consume
```

## 贡献

欢迎贡献代码、报告问题或提出功能建议！请查看我们的[贡献指南](CONTRIBUTING.md)了解更多信息。

## 许可证

本项目采用 MIT 许可证 - 详情请参阅 [LICENSE](LICENSE) 文件。
