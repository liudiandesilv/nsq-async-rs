# nsq-async-rs

[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

*其他语言版本: [English](README.md), [简体中文](README_zh.md)*

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

### 生产者示例

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

### 连接池

nsq-async-rs 提供内置的连接池实现，能够有效地管理和复用与 NSQ 的连接：

```rust
// 创建自定义连接池配置
let pool_config = ConnectionPoolConfig {
    max_connections_per_host: 10,
    max_idle_time: Duration::from_secs(60),
    health_check_interval: Duration::from_secs(30),
    // ... 其他配置
};

// 创建连接池
let pool = create_connection_pool(pool_config);

// 使用连接池创建生产者
let producer = new_producer(producer_config).with_connection_pool(pool);
```

## 贡献

欢迎贡献代码、报告问题或提出功能建议！请查看我们的[贡献指南](CONTRIBUTING.md)了解更多信息。

## 许可证

本项目采用 MIT 许可证 - 详情请参阅 [LICENSE](LICENSE) 文件。
