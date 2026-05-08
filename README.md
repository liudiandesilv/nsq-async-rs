# nsq-async-rs

[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Crates.io](https://img.shields.io/crates/v/nsq-async-rs.svg)](https://crates.io/crates/nsq-async-rs)

*Other language versions: [English](README.md), [简体中文](README_zh.md)*

nsq-async-rs is a high-performance, reliable NSQ client library written in Rust. The project is inspired by the official [go-nsq](https://github.com/nsqio/go-nsq) implementation and provides similar functionality and interfaces in the Rust ecosystem.

## Features

- ✨ Async I/O support (based on tokio)
- 🚀 High-performance message processing
- 🔄 Automatic reconnection and error retry
- 🔍 Support for nsqlookupd service discovery
- 🛡️ Graceful shutdown support
- 📊 Built-in message statistics
- ⚡ Support for delayed publishing
- 📦 Support for batch publishing
- 🔀 Support for concurrent message processing
- 🏊‍♂️ Built-in connection pool for producers
- 🎯 **Manual message acknowledgement support**
- 💫 Feature parity with official go-nsq

## Installation

Add the following to your `Cargo.toml`:

```toml
[dependencies]
nsq-async-rs = "0.1.9"
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
use nsq_async_rs::consumer::{Consumer, ConsumerConfig, Handler};
use nsq_async_rs::error::Result;
use nsq_async_rs::protocol::Message;
use std::time::Duration;

struct MessageHandler;

#[async_trait]
impl Handler for MessageHandler {
    async fn handle_message(&self, message: Message) -> Result<()> {
        let msg_id = message.id_string();
        // Add your message processing logic here
        println!("Processing message: {}", msg_id);
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // concurrent_handlers controls how many messages are processed in parallel
    let config = ConsumerConfig {
        max_in_flight: 100,
        concurrent_handlers: 20,
        ..Default::default()
    };

    let consumer = Consumer::new(
        "test_topic".to_string(),
        "test_channel".to_string(),
        config,
        MessageHandler,
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
use nsq_async_rs::producer::{NsqProducer, Producer, ProducerConfig};
use nsq_async_rs::error::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let producer = NsqProducer::new(ProducerConfig {
        nsqd_addresses: vec!["127.0.0.1:4150".to_string()],
        ..Default::default()
    });

    producer.publish("test_topic", b"Hello, NSQ!").await?;
    Ok(())
}
```

### Producer with Connection Pool Example

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
    // Configure the connection pool
    let pool_config = PoolConfig {
        initial_cap: 3,            // Initial connections
        max_cap: 10,               // Maximum connections
        max_idle: 5,               // Maximum idle connections
        idle_timeout: Duration::from_secs(30),  // Idle timeout
        max_lifetime: Duration::from_secs(300), // Connection lifetime (5 minutes)
    };

    // Create NSQ producer connection pool
    let pool = Pool::new(
        pool_config,
        // Factory function to create new connections
        || {
            let p_cfg = ProducerConfig {
                nsqd_addresses: vec!["127.0.0.1:4150".to_string()],
                ..Default::default()
            };
            Ok(new_producer(p_cfg))
        },
        // Close function (NSQ producers don't need explicit closing)
        |_producer| Ok(()),
        // Optional ping function to check connection health
        Some(|_producer| Ok(())),
    ).await?;

    // Get a connection from the pool
    let topic = "test_topic";
    let pooled_conn = pool.get().await?;
    
    // Use the connection
    match pooled_conn.conn.publish(topic, "Hello from connection pool!").await {
        Ok(_) => info!("Message published successfully"),
        Err(e) => error!("Failed to publish message: {}", e),
    }
    
    // Return the connection to the pool
    pool.put(pooled_conn).await?;
    
    // Close the pool when done
    pool.release().await?;
    
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
            "Message #{}, Time:{}",
            i + 1,
            Local::now().to_string()
        ));
    }

    // Measure batch publishing performance
    let start = Instant::now();
    producer.publish_multi(topic, messages).await?;
    let elapsed = start.elapsed();

    println!("Batch publishing 100 messages took: {:?}", elapsed);
    println!("Average time per message: {:?}", elapsed / 100);

    Ok(())
}
```

## Configuration Options

### Consumer Configuration

```rust
ConsumerConfig {
    max_in_flight: 100,                  // Maximum number of messages to process simultaneously
    max_attempts: 5,                     // Maximum number of retries
    dial_timeout: Duration::from_secs(1),  // Connection timeout
    read_timeout: Duration::from_secs(60), // Read timeout
    write_timeout: Duration::from_secs(1), // Write timeout
    lookup_poll_interval: Duration::from_secs(60), // nsqlookupd polling interval
    lookup_poll_jitter: 0.3,              // Polling jitter coefficient
    max_requeue_delay: Duration::from_secs(15 * 60), // Maximum requeue delay
    default_requeue_delay: Duration::from_secs(90),  // Default requeue delay
    shutdown_timeout: Duration::from_secs(30),       // Shutdown timeout
    backoff_strategy: true,               // Enable exponential backoff reconnection strategy
    disable_auto_response: false,         // Disable auto FIN/REQ; use manual ack
    concurrent_handlers: 1,              // Max concurrent handler tasks per connection
}
```

### Connection Pool Configuration

```rust
PoolConfig {
    initial_cap: 5,                    // Initial connections to create
    max_cap: 20,                       // Maximum connections allowed
    max_idle: 10,                      // Maximum idle connections to keep
    idle_timeout: Duration::from_secs(30), // How long connections can remain idle
    max_lifetime: Duration::from_secs(300), // Maximum connection lifetime (5 minutes)
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
    println!("NSQ server connection error: {}", err);
    // Handle connection error...
}
```

### Delayed Publishing

```rust
producer.publish_delayed("test_topic", b"Delayed message", Duration::from_secs(60)).await?;
```

### Batch Publishing

```rust
let messages: Vec<Vec<u8>> = vec![
    b"Message 1".to_vec(),
    b"Message 2".to_vec(),
    b"Message 3".to_vec(),
];
producer.publish_multi("test_topic", messages).await?;
```

## Manual Message Acknowledgement

nsq-async-rs supports manual message acknowledgement, giving you full control over when messages are acknowledged. This is particularly useful for concurrent processing, batch processing, or complex error handling scenarios.

### When to Use Manual Acknowledgement

Manual acknowledgement is ideal for:

1. **Concurrent message processing** - When you need to send messages to a worker thread pool for async processing
2. **Batch processing** - Accumulating multiple messages before processing them together
3. **Complex error handling** - Deciding whether to retry or discard based on different error types
4. **Long-running processing** - Using `TOUCH` to extend timeout for messages that take longer to process

### Enabling Manual Acknowledgement

```rust
let config = ConsumerConfig {
    max_in_flight: 100,
    disable_auto_response: true, // Enable manual acknowledgement
    ..Default::default()
};
```

### Manual Acknowledgement API

```rust
#[async_trait]
impl Handler for MyHandler {
    async fn handle_message(&self, message: Message) -> Result<()> {
        // Send to worker thread for async processing
        self.worker_tx.send(message).await?;
        
        // Return Ok, but won't auto-FIN (because manual ack is enabled)
        Ok(())
    }
}

// In worker thread
async fn worker(message: Message) {
    match process_message(&message.body).await {
        Ok(_) => {
            // Manually send FIN
            message.finish().await.unwrap();
        }
        Err(_) => {
            // Manually requeue with 5 second delay
            message.requeue(5000).await.unwrap();
        }
    }
}
```

### API Methods

- `message.finish()` - Send FIN command to mark message as successfully processed
- `message.requeue(delay)` - Send REQ command to requeue the message with a delay (in milliseconds)
- `message.touch()` - Send TOUCH command to reset message timeout
- `message.disable_auto_response()` - Disable auto-response for a specific message

### Complete Example

Here's a complete example showing manual acknowledgement with concurrent message processing:

```rust
use async_trait::async_trait;
use nsq_async_rs::consumer::{Consumer, ConsumerConfig, Handler};
use nsq_async_rs::protocol::Message;
use tokio::sync::mpsc;

struct ConcurrentHandler {
    worker_tx: mpsc::Sender<Message>,
}

impl ConcurrentHandler {
    fn new(worker_count: usize) -> Self {
        let (tx, mut rx) = mpsc::channel(100);
        
        // Start worker threads
        for worker_id in 0..worker_count {
            let mut worker_rx = rx.clone();
            tokio::spawn(async move {
                while let Some(msg) = worker_rx.recv().await {
                    // Process message
                    match process_message(&msg.body).await {
                        Ok(_) => {
                            msg.finish().await.ok();
                        }
                        Err(_) => {
                            if msg.attempts < 3 {
                                msg.requeue(5000).await.ok();
                            } else {
                                msg.finish().await.ok(); // Give up retrying
                            }
                        }
                    }
                }
            });
        }
        
        Self { worker_tx: tx }
    }
}

#[async_trait]
impl Handler for ConcurrentHandler {
    async fn handle_message(&self, message: Message) -> Result<()> {
        self.worker_tx.send(message).await?;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = ConsumerConfig {
        max_in_flight: 100,
        disable_auto_response: true, // Enable manual acknowledgement
        ..Default::default()
    };
    
    let handler = ConcurrentHandler::new(20);
    let consumer = Consumer::new("topic", "channel", config, handler)?;
    
    consumer.connect_to_nsqlookupd("http://127.0.0.1:4161").await?;
    consumer.start().await?;
    
    tokio::signal::ctrl_c().await?;
    consumer.stop().await?;
    
    Ok(())
}
```

### Running Examples

```bash
# Run manual acknowledgement example
cargo run --example manual_ack_consume

# Run automatic acknowledgement example (for comparison)
cargo run --example simple_consume
```

### Best Practices

1. **Ensure every message is acknowledged** - Use proper error handling to guarantee messages are acknowledged even when processing fails
2. **Set appropriate `max_in_flight`** - Can be set higher (e.g., 100) with manual ack, but ensure it doesn't exceed worker capacity
3. **Handle retry logic** - Check `message.attempts` to avoid infinite retries and use exponential backoff
4. **Avoid deadlocks** - Ensure message channels have sufficient buffer and use `try_send` or timeout mechanisms
5. **Monitor statistics** - Regularly call `consumer.stats()` to monitor `messages_received`, `messages_finished`, and `messages_requeued`

### Troubleshooting

**Message Timeout Issues**
- Symptom: Messages keep retrying, `attempts` keeps increasing
- Solution: Use `message.touch()` to reset timeout or increase `IdentifyConfig.msg_timeout`

**Message Loss**
- Symptom: Messages received but not processed
- Solution: Ensure manual acknowledgement is properly called in all code paths

**Memory Leaks**
- Symptom: Memory usage continuously grows
- Solution: Limit channel buffer size, monitor channel length, implement backpressure

## Contributing

Contributions are welcome! Please feel free to submit issues and pull requests.

## License

MIT License 