# nsq-async-rs

[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

*Read this in other languages: [English](README.md), [简体中文](README_zh.md)*

A high-performance, reliable NSQ client library written in Rust. This project provides similar functionality and interfaces to the official [go-nsq](https://github.com/nsqio/go-nsq) implementation within the Rust ecosystem.

## Features

- ✨ Asynchronous I/O support (based on tokio)
- 🚀 High-performance message processing
- 🔄 Automatic reconnection and error retry
- 🔍 Support for nsqlookupd service discovery
- 🛡️ Graceful shutdown support
- 📊 Built-in message statistics
- ⚡ Delayed publishing support
- 📦 Batch publishing support
- 💫 Feature parity with the official go-nsq client

## Installation

Add the following dependency to your `Cargo.toml` file:

```toml
[dependencies]
nsq-async-rs = "0.1.0"
```

## Quick Start

### Consumer Example

```rust
use nsq_async_rs::consumer::{Consumer, ConsumerConfig, Handler};
use nsq_async_rs::error::Result;
use nsq_async_rs::protocol::Message;

#[derive(Default)]
struct MessageHandler;

#[async_trait::async_trait]
impl Handler for MessageHandler {
    async fn handle_message(&self, message: Message) -> Result<()> {
        println!("Received message: {:?}", String::from_utf8_lossy(&message.body));
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

### Producer Example

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

## Configuration Options

### Consumer Configuration

```rust
ConsumerConfig {
    max_in_flight: 1,                     // Maximum number of messages to process simultaneously
    max_attempts: 5,                       // Maximum retry attempts
    dial_timeout: Duration::from_secs(1),  // Connection timeout
    read_timeout: Duration::from_secs(60), // Read timeout
    write_timeout: Duration::from_secs(1), // Write timeout
    lookup_poll_interval: Duration::from_secs(60),
    lookup_poll_jitter: 0.3,
    max_requeue_delay: Duration::from_secs(15 * 60),
    default_requeue_delay: Duration::from_secs(90),
    shutdown_timeout: Duration::from_secs(30),
}
```

## Advanced Features

### Delayed Publishing

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