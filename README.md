# nsq-async-rs

nsq-async-rs 是一个用 Rust 编写的 NSQ 客户端库，提供了高性能、可靠的消息队列客户端实现。该项目参考了官方的 [go-nsq](https://github.com/nsqio/go-nsq) 实现，并在 Rust 生态系统中提供了类似的功能和接口。

## 特性

- ✨ 异步 I/O 支持（基于 tokio）
- 🚀 高性能消息处理
- 🔄 自动重连和错误重试
- 🔍 支持 nsqlookupd 服务发现
- 🛡️ 优雅关闭支持
- 📊 内置消息统计
- ⚡ 支持延迟发布
- 📦 支持批量发布
- 💫 与官方 go-nsq 保持一致的功能特性

## 安装

将以下依赖添加到你的 `Cargo.toml` 文件中：

```toml
[dependencies]
nsq-async-rs = "0.1.0"
```

## 快速开始

### 消费者示例

```rust
use nsq-async-rs::consumer::{Consumer, ConsumerConfig, Handler};
use nsq-async-rs::error::Result;
use nsq-async-rs::protocol::Message;

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

### 生产者示例

```rust
use nsq-async-rs::producer::Producer;
use nsq-async-rs::error::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let producer = Producer::connect("127.0.0.1:4150").await?;
    
    producer.publish("test_topic", "Hello, NSQ!".as_bytes()).await?;
    Ok(())
}
```

## 配置选项

### 消费者配置

```rust
ConsumerConfig {
    max_in_flight: 1,                    // 同时处理的最大消息数
    max_attempts: 5,                     // 最大重试次数
    dial_timeout: Duration::from_secs(1),
    read_timeout: Duration::from_secs(60),
    write_timeout: Duration::from_secs(1),
    lookup_poll_interval: Duration::from_secs(60),
    lookup_poll_jitter: 0.3,
    max_requeue_delay: Duration::from_secs(15 * 60),
    default_requeue_delay: Duration::from_secs(90),
    shutdown_timeout: Duration::from_secs(30),
}
```

## 高级功能

### 延迟发布

```rust
producer.publish_with_delay("test_topic", "延迟消息".as_bytes(), 60).await?;
```

### 批量发布

```rust
let messages = vec![
    "消息1".as_bytes().to_vec(),
    "消息2".as_bytes().to_vec(),
];
producer.publish_multiple("test_topic", messages).await?;
```

## 错误处理

该库使用 `thiserror` 提供了详细的错误类型，包括：

- 连接错误
- 协议错误
- 超时错误
- 消息处理错误
- 配置错误

## 贡献

欢迎提交 Issue 和 Pull Request！

## 许可证

MIT License

## 实现说明

本项目在设计和实现时参考了 NSQ 官方的 Go 客户端库 [go-nsq](https://github.com/nsqio/go-nsq)，主要包括：

- 消息处理流程
- 连接管理机制
- 错误处理策略
- 配置参数设计
- 优雅关闭机制

我们在保持与 go-nsq 功能一致性的同时，充分利用了 Rust 语言的特性，提供了：

- 更严格的类型安全
- 基于 tokio 的异步支持
- Rust 风格的错误处理
- 更好的内存安全保证 