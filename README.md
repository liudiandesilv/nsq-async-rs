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
producer.publish_with_delay("test_topic", "Delayed message".as_bytes(), Duration::from_secs(60)).await?;
```

### Batch Publishing

```rust
let messages = vec![
    "Message 1".as_bytes().to_vec(),
    "Message 2".as_bytes().to_vec(),
];
producer.publish_multiple("test_topic", messages).await?;
```

## Error Handling

This library uses `thiserror` to provide detailed error types, including:

- Connection errors
- Protocol errors
- Timeout errors
- Message handling errors
- Configuration errors

## Connection Pool

nsq-async-rs includes a built-in connection pool implementation that efficiently manages and reuses NSQ connections:

```rust
// Create a custom connection pool configuration
let pool_config = ConnectionPoolConfig {
    max_connections_per_host: 10,
    max_idle_time: Duration::from_secs(60),
    health_check_interval: Duration::from_secs(30),
    // ... other configurations
};

// Create the connection pool
let pool = create_connection_pool(pool_config);

// Use the connection pool with a producer
let producer = new_producer(producer_config).with_connection_pool(pool);
```

## Contributing

Contributions are welcome! Please feel free to submit issues and pull requests.

## License

MIT License

## Implementation Notes

This project was designed and implemented with reference to NSQ's official Go client library [go-nsq](https://github.com/nsqio/go-nsq), including:

- Message processing flow
- Connection management mechanisms
- Error handling strategies
- Configuration parameter design
- Graceful shutdown mechanism

While maintaining functional parity with go-nsq, we've fully leveraged Rust language features to provide:

- Stricter type safety
- Asynchronous support based on tokio
- Rust-style error handling
- Improved memory safety guarantees 