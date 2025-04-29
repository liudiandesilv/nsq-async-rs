# nsq-async-rs

[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Crates.io](https://img.shields.io/crates/v/nsq-async-rs.svg)](https://crates.io/crates/nsq-async-rs)

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
- 🔀 Concurrent message processing
- 💫 Feature parity with the official go-nsq client

## Installation

Add the following dependency to your `Cargo.toml` file:

```toml
[dependencies]
nsq-async-rs = "0.1.3"
```

## Quick Start

### Basic Consumer Example

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

### Concurrent Consumer Example

```rust
use async_trait::async_trait;
use log::{error, info};
use nsq_async_rs::consumer::{Consumer, ConsumerConfig, Handler};
use nsq_async_rs::error::Result;
use nsq_async_rs::protocol::Message;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};

/// Concurrent message handler
struct ConcurrentMessageHandler {
    worker_count: usize,
    sender: Arc<Mutex<mpsc::Sender<Message>>>,
}

impl ConcurrentMessageHandler {
    pub fn new(worker_count: usize) -> Self {
        // Create message channel with buffer size 10x worker count
        let (tx, rx) = mpsc::channel(worker_count * 10);
        let sender = Arc::new(Mutex::new(tx));
        let receiver = Arc::new(Mutex::new(rx));

        let handler = Self {
            worker_count,
            sender,
        };

        // Start worker threads
        handler.start_workers(receiver);

        handler
    }

    fn start_workers(&self, receiver: Arc<Mutex<mpsc::Receiver<Message>>>) {
        for i in 0..self.worker_count {
            let worker_id = i + 1;
            let rx = receiver.clone();

            tokio::spawn(async move {
                info!("Worker {} started", worker_id);

                loop {
                    // Get message from channel
                    let msg = {
                        let mut rx_guard = rx.lock().await;
                        match rx_guard.recv().await {
                            Some(msg) => msg,
                            None => break,
                        }
                    };

                    // Process message
                    let msg_id = String::from_utf8_lossy(&msg.id).to_string();
                    info!("Worker {} processing message: {}", worker_id, msg_id);
                    
                    // Your message processing logic here
                    
                    info!("Worker {} completed message: {}", worker_id, msg_id);
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

        // Try non-blocking send first
        let send_result = sender.try_send(message.clone());
        match send_result {
            Ok(_) => {
                info!("Message sent to worker channel: ID={}", msg_id);
            }
            Err(mpsc::error::TrySendError::Full(msg)) => {
                // Channel full, use blocking send
                if let Err(e) = sender.send(msg).await {
                    error!("Failed to send message to worker channel: {}", e);
                    return Err(nsq_async_rs::error::Error::Other(e.to_string()));
                }
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                error!("Worker channel closed: ID={}", msg_id);
                return Err(nsq_async_rs::error::Error::Other("Worker channel closed".into()));
            }
        }

        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Create consumer config
    let config = ConsumerConfig {
        max_in_flight: 100, // Increase for better throughput
        max_attempts: 5,
        // other config options...
        ..Default::default()
    };

    // Create concurrent handler with 20 worker threads
    let handler = ConcurrentMessageHandler::new(20);

    // Create consumer
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

### Basic Producer Example

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

### Batch Publishing Example

```rust
use chrono::Local;
use nsq_async_rs::producer::{new_producer, ProducerConfig};
use std::error::Error;
use std::time::Instant;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // Create producer config
    let mut config = ProducerConfig::default();
    config.nsqd_addresses = vec!["127.0.0.1:4150".to_string()];

    // Create NSQ producer
    let producer = new_producer(config);
    let topic = "test_topic";
    
    // Prepare multiple messages
    let mut messages = vec![];
    for i in 0..100 {
        messages.push(format!(
            "Message #{},at:{}",
            i + 1,
            Local::now().to_string()
        ));
    }

    // Measure performance of batch publishing
    let start = Instant::now();
    producer.publish_multi(topic, messages).await?;
    let elapsed = start.elapsed();

    println!("Published 100 messages in {:?}", elapsed);
    println!("Average per message: {:?}", elapsed / 100);

    Ok(())
}
```

## Configuration Options

### Consumer Configuration

```rust
ConsumerConfig {
    max_in_flight: 100,                   // Maximum number of messages to process simultaneously
    max_attempts: 5,                       // Maximum retry attempts
    dial_timeout: Duration::from_secs(1),  // Connection timeout
    read_timeout: Duration::from_secs(60), // Read timeout
    write_timeout: Duration::from_secs(1), // Write timeout
    lookup_poll_interval: Duration::from_secs(60),
    lookup_poll_jitter: 0.3,
    max_requeue_delay: Duration::from_secs(15 * 60),
    default_requeue_delay: Duration::from_secs(90),
    shutdown_timeout: Duration::from_secs(30),
    backoff_strategy: true,                // Enable exponential backoff reconnection strategy
}
```

## Advanced Features

### Connection Health Check (Ping)

```rust
// Ping with default timeout (5 seconds)
let result = producer.ping(None, None).await;

// Ping with custom address and timeout
let result = producer.ping(
    Some("127.0.0.1:4150"),
    Some(Duration::from_millis(500))
).await;

// Check ping result before proceeding
if let Err(err) = result {
    println!("NSQ服务器连接异常: {}", err);
    // 处理连接异常...
}
```

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