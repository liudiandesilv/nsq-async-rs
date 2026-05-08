use async_trait::async_trait;
use nsq_async_rs::consumer::{Consumer, ConsumerConfig, Handler};
use nsq_async_rs::error::Result;
use nsq_async_rs::producer::{NsqProducer, Producer, ProducerConfig};
use nsq_async_rs::protocol::Message;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::time::sleep;

const NSQD: &str = "127.0.0.1:4150";

fn make_producer() -> NsqProducer {
    NsqProducer::new(ProducerConfig {
        nsqd_addresses: vec![NSQD.to_string()],
        ..Default::default()
    })
}

fn make_consumer_config(max_in_flight: i32, concurrent: usize) -> ConsumerConfig {
    ConsumerConfig {
        max_in_flight,
        concurrent_handlers: concurrent,
        read_timeout: Duration::from_secs(5),
        write_timeout: Duration::from_secs(2),
        shutdown_timeout: Duration::from_secs(3),
        backoff_strategy: false,
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

struct CountingHandler(Arc<AtomicUsize>);

#[async_trait]
impl Handler for CountingHandler {
    async fn handle_message(&self, _msg: Message) -> Result<()> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

struct ManualAckCountingHandler(Arc<AtomicUsize>);

#[async_trait]
impl Handler for ManualAckCountingHandler {
    async fn handle_message(&self, msg: Message) -> Result<()> {
        msg.finish().await?;
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helper: wait until counter reaches target or timeout
// ---------------------------------------------------------------------------

async fn wait_for_count(counter: &Arc<AtomicUsize>, target: usize, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if counter.load(Ordering::Relaxed) >= target {
            return true;
        }
        sleep(Duration::from_millis(50)).await;
    }
    false
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Publish a single message to nsqd.
#[tokio::test]
#[ignore = "requires local nsqd at 127.0.0.1:4150"]
async fn publish_single_message() {
    let p = make_producer();
    p.publish("nsq_test_pub", b"hello nsq").await.unwrap();
}

/// Publish 100 messages in a single batch.
#[tokio::test]
#[ignore = "requires local nsqd at 127.0.0.1:4150"]
async fn publish_batch_messages() {
    let p = make_producer();
    let msgs: Vec<Vec<u8>> = (0..100).map(|i| format!("msg-{i}").into_bytes()).collect();
    p.publish_multi("nsq_test_batch", msgs).await.unwrap();
}

/// Publish 10 messages then consume them with auto-FIN.
#[tokio::test]
#[ignore = "requires local nsqd at 127.0.0.1:4150"]
async fn publish_then_consume_messages() {
    let topic = "nsq_test_e2e";
    let p = make_producer();
    for i in 0..10u32 {
        p.publish(topic, format!("msg-{i}").as_bytes()).await.unwrap();
    }

    let counter = Arc::new(AtomicUsize::new(0));
    let consumer = Consumer::new(
        topic.to_string(),
        "test_channel".to_string(),
        make_consumer_config(10, 10),
        CountingHandler(Arc::clone(&counter)),
    )
    .unwrap();

    consumer.connect_to_nsqd(NSQD.to_string()).await.unwrap();

    assert!(
        wait_for_count(&counter, 10, Duration::from_secs(10)).await,
        "timed out waiting for 10 messages, got {}",
        counter.load(Ordering::Relaxed)
    );

    let stats = consumer.stats();
    assert_eq!(stats.messages_finished, 10);
    consumer.stop().await.unwrap();
}

/// Publish 5 messages then consume with manual ACK.
#[tokio::test]
#[ignore = "requires local nsqd at 127.0.0.1:4150"]
async fn publish_then_consume_with_manual_ack() {
    let topic = "nsq_test_manual_ack";
    let p = make_producer();
    for i in 0..5u32 {
        p.publish(topic, format!("msg-{i}").as_bytes()).await.unwrap();
    }

    let counter = Arc::new(AtomicUsize::new(0));
    let config = ConsumerConfig {
        disable_auto_response: true,
        ..make_consumer_config(5, 5)
    };
    let consumer = Consumer::new(
        topic.to_string(),
        "test_channel".to_string(),
        config,
        ManualAckCountingHandler(Arc::clone(&counter)),
    )
    .unwrap();

    consumer.connect_to_nsqd(NSQD.to_string()).await.unwrap();

    assert!(
        wait_for_count(&counter, 5, Duration::from_secs(10)).await,
        "timed out waiting for 5 messages, got {}",
        counter.load(Ordering::Relaxed)
    );

    let stats = consumer.stats();
    assert_eq!(stats.messages_finished, 5);
    assert_eq!(stats.messages_requeued, 0);
    consumer.stop().await.unwrap();
}

/// Throughput test: 10 concurrent producers × 1000 messages = 10000 total.
/// Consumer: max_in_flight=100, concurrent_handlers=50.
/// Prints publish TPS and consume TPS; asserts all messages consumed.
#[tokio::test]
#[ignore = "requires local nsqd at 127.0.0.1:4150"]
async fn throughput_publish_and_consume() {
    let topic = "nsq_test_throughput";
    const TOTAL: usize = 10_000;
    const PRODUCERS: usize = 10;
    const PER_PRODUCER: usize = TOTAL / PRODUCERS;

    let counter = Arc::new(AtomicUsize::new(0));
    let consumer = Consumer::new(
        topic.to_string(),
        "test_channel".to_string(),
        make_consumer_config(100, 50),
        CountingHandler(Arc::clone(&counter)),
    )
    .unwrap();
    consumer.connect_to_nsqd(NSQD.to_string()).await.unwrap();

    // publish concurrently
    let pub_start = Instant::now();
    let producer = Arc::new(make_producer());
    let mut handles = Vec::with_capacity(PRODUCERS);
    for t in 0..PRODUCERS {
        let p = Arc::clone(&producer);
        handles.push(tokio::spawn(async move {
            for i in 0..PER_PRODUCER {
                p.publish(topic, format!("t{t}-m{i}").as_bytes())
                    .await
                    .unwrap();
            }
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    let pub_elapsed = pub_start.elapsed();
    let pub_tps = TOTAL as f64 / pub_elapsed.as_secs_f64();

    // wait for all messages consumed
    let consume_start = Instant::now();
    assert!(
        wait_for_count(&counter, TOTAL, Duration::from_secs(60)).await,
        "timed out: consumed {} / {}",
        counter.load(Ordering::Relaxed),
        TOTAL
    );
    let consume_elapsed = consume_start.elapsed();
    let consume_tps = TOTAL as f64 / consume_elapsed.as_secs_f64();

    println!(
        "\n=== Throughput ===\n  publish : {TOTAL} msgs in {pub_elapsed:.2?}  →  {pub_tps:.0} msg/s\n  consume : {TOTAL} msgs in {consume_elapsed:.2?}  →  {consume_tps:.0} msg/s"
    );

    let stats = consumer.stats();
    assert_eq!(stats.messages_finished as usize, TOTAL);
    consumer.stop().await.unwrap();
}
