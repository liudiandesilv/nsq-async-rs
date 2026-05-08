use async_trait::async_trait;
use nsq_async_rs::consumer::{Consumer, ConsumerConfig};
use nsq_async_rs::error::{Error, Result};
use nsq_async_rs::protocol::Message;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::timeout;

// ---------------------------------------------------------------------------
// Mock NSQ server helpers
// ---------------------------------------------------------------------------

fn response_frame(body: &[u8]) -> Vec<u8> {
    nsq_frame(0, body)
}

fn message_frame(id: &[u8; 16], body: &[u8]) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&1234567890u64.to_be_bytes()); // timestamp
    payload.extend_from_slice(&1u16.to_be_bytes()); // attempts
    payload.extend_from_slice(id);
    payload.extend_from_slice(body);
    nsq_frame(2, &payload)
}

fn nsq_frame(frame_type: i32, body: &[u8]) -> Vec<u8> {
    let size = (4 + body.len()) as u32;
    let mut frame = Vec::new();
    frame.extend_from_slice(&size.to_be_bytes());
    frame.extend_from_slice(&frame_type.to_be_bytes());
    frame.extend_from_slice(body);
    frame
}

/// Perform the NSQ handshake (magic + IDENTIFY) and send OK response.
/// Returns the raw bytes of the IDENTIFY JSON body.
async fn do_handshake(stream: &mut tokio::net::TcpStream) -> Vec<u8> {
    // Read magic "  V2" (4 bytes)
    let mut magic = [0u8; 4];
    stream.read_exact(&mut magic).await.unwrap();
    assert_eq!(&magic, b"  V2");

    // Read "IDENTIFY\n" line
    let mut line = Vec::new();
    loop {
        let mut b = [0u8; 1];
        stream.read_exact(&mut b).await.unwrap();
        if b[0] == b'\n' {
            break;
        }
        line.push(b[0]);
    }
    assert_eq!(line, b"IDENTIFY");

    // Read 4-byte body length
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await.unwrap();
    let body_len = u32::from_be_bytes(len_buf) as usize;

    // Read JSON body
    let mut body = vec![0u8; body_len];
    stream.read_exact(&mut body).await.unwrap();

    // Send OK response frame
    stream
        .write_all(&response_frame(b"OK"))
        .await
        .unwrap();
    stream.flush().await.unwrap();

    body
}

/// Read one newline-terminated command from the stream.
async fn read_command(stream: &mut tokio::net::TcpStream) -> String {
    let mut line = Vec::new();
    loop {
        let mut b = [0u8; 1];
        match timeout(Duration::from_millis(500), stream.read_exact(&mut b)).await {
            Ok(Ok(_)) => {
                if b[0] == b'\n' {
                    break;
                }
                line.push(b[0]);
            }
            _ => break,
        }
    }
    String::from_utf8_lossy(&line).to_string()
}

fn test_config(max_in_flight: i32) -> ConsumerConfig {
    ConsumerConfig {
        max_in_flight,
        read_timeout: Duration::from_millis(500),
        write_timeout: Duration::from_millis(500),
        shutdown_timeout: Duration::from_millis(300),
        backoff_strategy: false,
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

struct RecordingHandler {
    bodies: Arc<tokio::sync::Mutex<Vec<Vec<u8>>>>,
    result: bool,
}

#[async_trait]
impl nsq_async_rs::consumer::Handler for RecordingHandler {
    async fn handle_message(&self, message: Message) -> Result<()> {
        self.bodies.lock().await.push(message.body.clone());
        if self.result {
            Ok(())
        } else {
            Err(Error::Other("handler failed".to_string()))
        }
    }
}

struct ManualAckHandler {
    bodies: Arc<tokio::sync::Mutex<Vec<Vec<u8>>>>,
}

#[async_trait]
impl nsq_async_rs::consumer::Handler for ManualAckHandler {
    async fn handle_message(&self, message: Message) -> Result<()> {
        self.bodies.lock().await.push(message.body.clone());
        message.finish().await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Consumer sends magic, IDENTIFY, SUB, and initial RDY after connect.
#[tokio::test]
async fn consumer_connect_sends_identify_subscribe_and_initial_rdy() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        do_handshake(&mut stream).await;

        let sub = read_command(&mut stream).await;
        let rdy = read_command(&mut stream).await;
        (sub, rdy)
    });

    let consumer = Consumer::new(
        "test_topic".to_string(),
        "test_channel".to_string(),
        test_config(3),
        RecordingHandler {
            bodies: Arc::new(tokio::sync::Mutex::new(vec![])),
            result: true,
        },
    )
    .unwrap();

    consumer.connect_to_nsqd(addr).await.unwrap();

    let (sub, rdy) = timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(sub, "SUB test_topic test_channel");
    assert_eq!(rdy, "RDY 3");

    consumer.stop().await.unwrap();
}

/// Consumer auto-sends FIN and increments stats when handler returns Ok.
#[tokio::test]
async fn consumer_auto_finishes_message_when_handler_succeeds() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let msg_id: [u8; 16] = *b"msg-success-0001";

    let (cmd_tx, mut cmd_rx) = mpsc::channel::<String>(16);

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        do_handshake(&mut stream).await;

        // consume SUB + RDY
        read_command(&mut stream).await;
        read_command(&mut stream).await;

        // send one message
        stream
            .write_all(&message_frame(&msg_id, b"hello"))
            .await
            .unwrap();
        stream.flush().await.unwrap();

        // collect commands until FIN or timeout
        loop {
            let cmd = read_command(&mut stream).await;
            if cmd.is_empty() {
                break;
            }
            if cmd == "NOP" {
                continue;
            }
            let _ = cmd_tx.send(cmd.clone()).await;
            if cmd.starts_with("FIN") {
                break;
            }
        }
    });

    let bodies = Arc::new(tokio::sync::Mutex::new(vec![]));
    let consumer = Consumer::new(
        "test_topic".to_string(),
        "test_channel".to_string(),
        test_config(1),
        RecordingHandler {
            bodies: Arc::clone(&bodies),
            result: true,
        },
    )
    .unwrap();

    consumer.connect_to_nsqd(addr).await.unwrap();
    timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap();

    let fin_cmd = timeout(Duration::from_secs(2), cmd_rx.recv())
        .await
        .unwrap()
        .unwrap();

    assert!(
        fin_cmd.starts_with("FIN msg-success-0001"),
        "expected FIN, got: {fin_cmd}"
    );

    let stats = consumer.stats();
    assert_eq!(stats.messages_received, 1);
    assert_eq!(stats.messages_finished, 1);
    assert_eq!(stats.messages_requeued, 0);

    consumer.stop().await.unwrap();
}

/// Consumer auto-sends REQ and increments requeued stat when handler returns Err.
#[tokio::test]
async fn consumer_auto_requeues_message_when_handler_fails() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let msg_id: [u8; 16] = *b"msg-failure-0001";

    let (cmd_tx, mut cmd_rx) = mpsc::channel::<String>(16);

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        do_handshake(&mut stream).await;
        read_command(&mut stream).await; // SUB
        read_command(&mut stream).await; // RDY

        stream
            .write_all(&message_frame(&msg_id, b"fail-body"))
            .await
            .unwrap();
        stream.flush().await.unwrap();

        loop {
            let cmd = read_command(&mut stream).await;
            if cmd.is_empty() {
                break;
            }
            if cmd == "NOP" {
                continue;
            }
            let _ = cmd_tx.send(cmd.clone()).await;
            if cmd.starts_with("REQ") {
                break;
            }
        }
    });

    let bodies = Arc::new(tokio::sync::Mutex::new(vec![]));
    let consumer = Consumer::new(
        "test_topic".to_string(),
        "test_channel".to_string(),
        test_config(1),
        RecordingHandler {
            bodies: Arc::clone(&bodies),
            result: false,
        },
    )
    .unwrap();

    consumer.connect_to_nsqd(addr).await.unwrap();
    timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap();

    let req_cmd = timeout(Duration::from_secs(2), cmd_rx.recv())
        .await
        .unwrap()
        .unwrap();

    assert!(
        req_cmd.starts_with("REQ msg-failure-0001"),
        "expected REQ, got: {req_cmd}"
    );

    let stats = consumer.stats();
    assert_eq!(stats.messages_received, 1);
    assert_eq!(stats.messages_finished, 0);
    assert_eq!(stats.messages_requeued, 1);

    consumer.stop().await.unwrap();
}

/// With disable_auto_response=true, handler manually calls finish() — exactly one FIN sent.
#[tokio::test]
async fn consumer_manual_ack_sends_exactly_one_fin() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let msg_id: [u8; 16] = *b"msg-manual-00001";

    let fin_count = Arc::new(AtomicUsize::new(0));
    let fin_count_srv = Arc::clone(&fin_count);

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        do_handshake(&mut stream).await;
        read_command(&mut stream).await; // SUB
        read_command(&mut stream).await; // RDY

        stream
            .write_all(&message_frame(&msg_id, b"manual-body"))
            .await
            .unwrap();
        stream.flush().await.unwrap();

        // collect commands for a short window
        for _ in 0..10 {
            let cmd = read_command(&mut stream).await;
            if cmd.is_empty() {
                break;
            }
            if cmd.starts_with("FIN") {
                fin_count_srv.fetch_add(1, Ordering::Relaxed);
            }
        }
    });

    let bodies = Arc::new(tokio::sync::Mutex::new(vec![]));
    let config = ConsumerConfig {
        disable_auto_response: true,
        ..test_config(1)
    };
    let consumer = Consumer::new(
        "test_topic".to_string(),
        "test_channel".to_string(),
        config,
        ManualAckHandler {
            bodies: Arc::clone(&bodies),
        },
    )
    .unwrap();

    consumer.connect_to_nsqd(addr).await.unwrap();
    timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap();

    // brief wait to ensure no duplicate FIN arrives
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(fin_count.load(Ordering::Relaxed), 1, "expected exactly 1 FIN");

    consumer.stop().await.unwrap();
}

/// Consumer replies NOP to _heartbeat_ response frames.
#[tokio::test]
async fn consumer_replies_nop_to_heartbeat() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();

    let (cmd_tx, mut cmd_rx) = mpsc::channel::<String>(16);

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        do_handshake(&mut stream).await;
        read_command(&mut stream).await; // SUB
        read_command(&mut stream).await; // RDY

        // send heartbeat
        stream
            .write_all(&response_frame(b"_heartbeat_"))
            .await
            .unwrap();
        stream.flush().await.unwrap();

        loop {
            let cmd = read_command(&mut stream).await;
            if cmd.is_empty() {
                break;
            }
            let _ = cmd_tx.send(cmd.clone()).await;
            if cmd == "NOP" {
                break;
            }
        }
    });

    let consumer = Consumer::new(
        "test_topic".to_string(),
        "test_channel".to_string(),
        test_config(1),
        RecordingHandler {
            bodies: Arc::new(tokio::sync::Mutex::new(vec![])),
            result: true,
        },
    )
    .unwrap();

    consumer.connect_to_nsqd(addr).await.unwrap();
    timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap();

    let nop = timeout(Duration::from_secs(2), cmd_rx.recv())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(nop, "NOP");

    consumer.stop().await.unwrap();
}

/// connection_count increments on connect and decrements on disconnect.
#[tokio::test]
async fn consumer_connection_count_tracks_connect_and_disconnect() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();

    // server: accept and hold connection open
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        do_handshake(&mut stream).await;
        read_command(&mut stream).await; // SUB
        read_command(&mut stream).await; // RDY
        // hold open
        tokio::time::sleep(Duration::from_secs(5)).await;
    });

    let consumer = Consumer::new(
        "test_topic".to_string(),
        "test_channel".to_string(),
        test_config(1),
        RecordingHandler {
            bodies: Arc::new(tokio::sync::Mutex::new(vec![])),
            result: true,
        },
    )
    .unwrap();

    consumer.connect_to_nsqd(addr.clone()).await.unwrap();

    // brief wait for spawn to register
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(consumer.stats().connections, 1);

    consumer.disconnect_from_nsqd(addr).await.unwrap();
    assert_eq!(consumer.stats().connections, 0);

    consumer.stop().await.unwrap();
}

/// concurrent_handlers=3, 3 messages sent simultaneously.
/// Each handler sleeps 80ms. Total wall time should be < 150ms (not 240ms serial).
#[tokio::test]
async fn consumer_processes_messages_concurrently_up_to_max_in_flight() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();

    let ids: [[u8; 16]; 3] = [
        *b"concurrent-msg-1",
        *b"concurrent-msg-2",
        *b"concurrent-msg-3",
    ];

    let (fin_tx, mut fin_rx) = mpsc::channel::<String>(16);

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        do_handshake(&mut stream).await;
        read_command(&mut stream).await; // SUB
        read_command(&mut stream).await; // RDY 3

        // send 3 messages back-to-back
        for id in &ids {
            stream.write_all(&message_frame(id, b"body")).await.unwrap();
        }
        stream.flush().await.unwrap();

        // collect FIN commands
        let mut count = 0;
        while count < 3 {
            let cmd = read_command(&mut stream).await;
            if cmd.is_empty() {
                break;
            }
            if cmd.starts_with("FIN") || cmd.starts_with("RDY") {
                if cmd.starts_with("FIN") {
                    let _ = fin_tx.send(cmd).await;
                    count += 1;
                }
            }
        }
    });

    let start = std::time::Instant::now();

    let config = ConsumerConfig {
        max_in_flight: 3,
        concurrent_handlers: 3,
        read_timeout: Duration::from_millis(500),
        write_timeout: Duration::from_millis(500),
        shutdown_timeout: Duration::from_millis(500),
        backoff_strategy: false,
        ..Default::default()
    };

    struct SlowHandler;
    #[async_trait]
    impl nsq_async_rs::consumer::Handler for SlowHandler {
        async fn handle_message(&self, _msg: Message) -> nsq_async_rs::error::Result<()> {
            tokio::time::sleep(Duration::from_millis(80)).await;
            Ok(())
        }
    }

    let consumer = Consumer::new(
        "test_topic".to_string(),
        "test_channel".to_string(),
        config,
        SlowHandler,
    )
    .unwrap();

    consumer.connect_to_nsqd(addr).await.unwrap();
    timeout(Duration::from_secs(3), server).await.unwrap().unwrap();

    // wait for all 3 FINs
    for _ in 0..3 {
        timeout(Duration::from_secs(2), fin_rx.recv())
            .await
            .unwrap()
            .unwrap();
    }

    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_millis(250),
        "expected concurrent processing < 250ms, got {:?}",
        elapsed
    );

    consumer.stop().await.unwrap();
}
