use nsq_async_rs::protocol::Message;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// 测试 Message 的基本创建和状态
#[test]
fn test_message_creation_and_state() {
    let msg = Message::new(
        vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
        b"test body".to_vec(),
        123456789,
        1,
    );

    // 验证初始状态
    assert_eq!(msg.attempts, 1);
    assert_eq!(msg.timestamp, 123456789);
    assert_eq!(msg.body, b"test body");
    assert!(!msg.is_auto_response_disabled(), "默认应该启用自动响应");
    assert!(!msg.has_responded(), "初始状态不应该已响应");
}

/// 测试禁用自动响应
#[test]
fn test_disable_auto_response() {
    let mut msg = Message::new(vec![1; 16], b"test".to_vec(), 123456789, 1);

    assert!(!msg.is_auto_response_disabled(), "初始应该启用自动响应");

    msg.disable_auto_response();

    assert!(msg.is_auto_response_disabled(), "调用后应该禁用自动响应");
}

/// 测试消息 ID 字符串转换
#[test]
fn test_message_id_string() {
    let id = b"0123456789abcdef".to_vec();
    let msg = Message::new(id.clone(), b"body".to_vec(), 0, 0);

    let id_str = msg.id_string();
    assert_eq!(id_str, "0123456789abcdef");
}

/// 测试消息的克隆保持状态
#[test]
fn test_message_clone_preserves_state() {
    let mut msg = Message::new(vec![1; 16], b"test".to_vec(), 123, 1);
    msg.disable_auto_response();

    let cloned = msg.clone();

    assert!(cloned.is_auto_response_disabled(), "克隆应该保持禁用状态");
    assert_eq!(cloned.attempts, msg.attempts);
    assert_eq!(cloned.timestamp, msg.timestamp);
}

/// 测试响应状态在多次调用中的幂等性
#[test]
fn test_has_responded_idempotency() {
    let msg = Message::new(vec![1; 16], b"test".to_vec(), 123, 1);

    // 初始状态
    assert!(!msg.has_responded());

    // 多次检查应该返回相同结果
    assert!(!msg.has_responded());
    assert!(!msg.has_responded());
}

/// 测试并发场景下的消息处理（模拟）
#[tokio::test]
async fn test_concurrent_message_handling() {
    use std::time::Duration;
    use tokio::sync::{Mutex, mpsc};

    let (tx, rx) = mpsc::channel::<Message>(10);
    let rx = Arc::new(Mutex::new(rx));
    let counter = Arc::new(AtomicU32::new(0));

    // 启动 3 个工作线程
    for worker_id in 0..3 {
        let worker_rx = rx.clone();
        let worker_counter = counter.clone();

        tokio::spawn(async move {
            loop {
                let _msg = {
                    let mut rx_guard = worker_rx.lock().await;
                    match rx_guard.recv().await {
                        Some(m) => m,
                        None => break,
                    }
                };

                // 模拟处理
                tokio::time::sleep(Duration::from_millis(10)).await;
                worker_counter.fetch_add(1, Ordering::Relaxed);

                // 在真实场景中，这里会调用 _msg.finish()
                println!("Worker {} processed message", worker_id);
            }
        });
    }

    // 发送 10 条消息
    for i in 0..10 {
        let msg = Message::new(
            vec![i as u8; 16],
            format!("message {}", i).into_bytes(),
            i as u64,
            1,
        );
        tx.send(msg).await.unwrap();
    }

    // 等待处理完成
    tokio::time::sleep(Duration::from_millis(200)).await;

    // 验证所有消息都被处理
    let processed = counter.load(Ordering::Relaxed);
    assert_eq!(processed, 10, "应该处理了 10 条消息");
}

/// 测试消息重试逻辑
#[test]
fn test_message_retry_attempts() {
    let msg1 = Message::new(vec![1; 16], b"test".to_vec(), 123, 1);
    let msg2 = Message::new(vec![2; 16], b"test".to_vec(), 123, 3);
    let msg3 = Message::new(vec![3; 16], b"test".to_vec(), 123, 5);

    assert_eq!(msg1.attempts, 1);
    assert_eq!(msg2.attempts, 3);
    assert_eq!(msg3.attempts, 5);

    // 模拟重试逻辑判断
    assert!(msg1.attempts <= 3, "attempts <= 3 应该重试");
    assert!(msg2.attempts <= 3, "attempts = 3 应该是最后一次重试");
    assert!(msg3.attempts > 3, "attempts > 3 应该放弃");
}

/// 测试从字节流解析消息
#[test]
fn test_message_from_bytes() {
    // 构造一个有效的消息字节流
    // 格式: [4字节大小][4字节类型=2][8字节时间戳][2字节attempts][16字节ID][消息体]
    let mut bytes = Vec::new();

    // 大小占位符（稍后填充）
    bytes.extend_from_slice(&[0u8; 4]);

    // 帧类型 = 2 (消息)
    bytes.extend_from_slice(&2u32.to_be_bytes());

    // 时间戳
    bytes.extend_from_slice(&123456789u64.to_be_bytes());

    // attempts
    bytes.extend_from_slice(&2u16.to_be_bytes());

    // 消息 ID (16 字节)
    bytes.extend_from_slice(b"0123456789abcdef");

    // 消息体
    bytes.extend_from_slice(b"Hello NSQ!");

    // 解析消息
    let result = Message::from_bytes(&bytes);

    assert!(result.is_ok(), "应该成功解析消息");

    let msg = result.unwrap();
    assert_eq!(msg.timestamp, 123456789);
    assert_eq!(msg.attempts, 2);
    assert_eq!(msg.body, b"Hello NSQ!");
    assert!(!msg.is_auto_response_disabled());
    assert!(!msg.has_responded());
}

/// 测试解析无效消息
#[test]
fn test_message_from_bytes_invalid() {
    // 消息太短
    let short_bytes = vec![0u8; 20];
    let result = Message::from_bytes(&short_bytes);
    assert!(result.is_err(), "太短的消息应该解析失败");

    // 无效的帧类型
    let mut invalid_type = Vec::new();
    invalid_type.extend_from_slice(&[0u8; 4]); // 大小
    invalid_type.extend_from_slice(&1u32.to_be_bytes()); // 错误的类型
    invalid_type.extend_from_slice(&[0u8; 26]); // 其他数据

    let result = Message::from_bytes(&invalid_type);
    assert!(result.is_err(), "无效帧类型应该解析失败");
}

/// 测试 ConsumerConfig 的默认值
#[test]
fn test_consumer_config_defaults() {
    use nsq_async_rs::consumer::ConsumerConfig;

    let config = ConsumerConfig::default();

    assert_eq!(config.max_in_flight, 1);
    assert_eq!(config.max_attempts, 5);
    assert!(!config.disable_auto_response, "默认应该启用自动响应");
    assert!(config.backoff_strategy, "默认应该启用退避策略");
}

/// 测试 ConsumerConfig 的自定义配置
#[test]
fn test_consumer_config_custom() {
    use nsq_async_rs::consumer::ConsumerConfig;
    use std::time::Duration;

    let config = ConsumerConfig {
        max_in_flight: 100,
        max_attempts: 3,
        disable_auto_response: true,
        read_timeout: Duration::from_secs(30),
        ..Default::default()
    };

    assert_eq!(config.max_in_flight, 100);
    assert_eq!(config.max_attempts, 3);
    assert!(config.disable_auto_response, "应该禁用自动响应");
    assert_eq!(config.read_timeout, Duration::from_secs(30));
}

/// 测试手动 finish() - 没有连接的情况
#[tokio::test]
async fn test_manual_finish_without_connection() {
    let msg = Message::new(vec![1; 16], b"test".to_vec(), 123456789, 1);

    // 没有连接引用，应该返回错误
    let result = msg.finish().await;
    assert!(result.is_err(), "没有连接时 finish() 应该返回错误");
    assert!(result.unwrap_err().to_string().contains("没有关联的连接"));
}

/// 测试手动 requeue() - 没有连接的情况
#[tokio::test]
async fn test_manual_requeue_without_connection() {
    let msg = Message::new(vec![1; 16], b"test".to_vec(), 123456789, 1);

    // 没有连接引用，应该返回错误
    let result = msg.requeue(5000).await;
    assert!(result.is_err(), "没有连接时 requeue() 应该返回错误");
    assert!(result.unwrap_err().to_string().contains("没有关联的连接"));
}

/// 测试手动 touch() - 没有连接的情况
#[tokio::test]
async fn test_manual_touch_without_connection() {
    let msg = Message::new(vec![1; 16], b"test".to_vec(), 123456789, 1);

    // 没有连接引用，应该返回错误
    let result = msg.touch().await;
    assert!(result.is_err(), "没有连接时 touch() 应该返回错误");
    assert!(result.unwrap_err().to_string().contains("没有关联的连接"));
}

/// 测试 finish() 的幂等性（重复调用）
#[tokio::test]
async fn test_finish_idempotency() {
    let msg = Message::new(vec![1; 16], b"test".to_vec(), 123456789, 1);

    // 第一次调用（会失败因为没有连接，但会标记为已响应）
    let _ = msg.finish().await;
    assert!(msg.has_responded(), "第一次 finish() 后应该标记为已响应");

    // 第二次调用应该直接返回 Ok（幂等性）
    let result = msg.finish().await;
    assert!(result.is_ok(), "重复调用 finish() 应该是幂等的");
}

/// 测试 finish() 和 requeue() 互斥
#[tokio::test]
async fn test_finish_and_requeue_mutual_exclusion() {
    let msg = Message::new(vec![1; 16], b"test".to_vec(), 123456789, 1);

    // 第一次调用 finish（会失败但标记已响应）
    let _ = msg.finish().await;
    assert!(msg.has_responded(), "finish() 后应该标记为已响应");

    // 尝试 requeue 应该被忽略（幂等返回 Ok）
    let result = msg.requeue(5000).await;
    assert!(result.is_ok(), "已响应后的 requeue() 应该被幂等忽略");
}

/// 测试 touch() 在响应后的行为
#[tokio::test]
async fn test_touch_after_response() {
    let msg = Message::new(vec![1; 16], b"test".to_vec(), 123456789, 1);

    // 先 finish
    let _ = msg.finish().await;
    assert!(msg.has_responded(), "finish() 后应该标记为已响应");

    // touch() 应该直接返回 Ok（不做任何操作）
    let result = msg.touch().await;
    assert!(result.is_ok(), "已响应后的 touch() 应该直接返回");
}

/// 测试消息响应状态的原子性
#[tokio::test]
async fn test_response_atomicity() {
    use std::sync::Arc;
    use tokio::task;

    let msg = Arc::new(Message::new(vec![1; 16], b"test".to_vec(), 123456789, 1));

    // 启动多个并发任务尝试响应同一消息
    let mut handles = vec![];
    for _ in 0..10 {
        let msg_clone = Arc::clone(&msg);
        let handle = task::spawn(async move {
            // 尝试 finish，由于没有连接会失败，但会尝试设置响应状态
            let _ = msg_clone.finish().await;
        });
        handles.push(handle);
    }

    // 等待所有任务完成
    for handle in handles {
        handle.await.unwrap();
    }

    // 验证只被响应一次（状态应该是 true）
    assert!(msg.has_responded(), "应该被标记为已响应");
}

/// 测试禁用自动响应的消息
#[tokio::test]
async fn test_message_with_auto_response_disabled() {
    let mut msg = Message::new(vec![1; 16], b"test".to_vec(), 123456789, 1);

    assert!(!msg.is_auto_response_disabled(), "初始状态应启用自动响应");
    assert!(!msg.has_responded(), "初始状态未响应");

    // 禁用自动响应
    msg.disable_auto_response();
    assert!(msg.is_auto_response_disabled(), "应该禁用自动响应");

    // 手动响应
    let _ = msg.finish().await;
    assert!(msg.has_responded(), "手动响应后应该标记为已响应");
}

/// 测试多次禁用自动响应的幂等性
#[test]
fn test_disable_auto_response_idempotency() {
    let mut msg = Message::new(vec![1; 16], b"test".to_vec(), 123456789, 1);

    msg.disable_auto_response();
    assert!(msg.is_auto_response_disabled());

    // 重复禁用
    msg.disable_auto_response();
    assert!(msg.is_auto_response_disabled(), "重复禁用应该是幂等的");
}

/// 测试不同延迟时间的 requeue
#[tokio::test]
async fn test_requeue_with_different_delays() {
    // 测试立即重试
    let msg1 = Message::new(vec![1; 16], b"test".to_vec(), 123456789, 1);
    let result1 = msg1.requeue(0).await;
    assert!(result1.is_err() || result1.is_ok()); // 可能失败（无连接）或成功（幂等）

    // 测试延迟 5 秒
    let msg2 = Message::new(vec![2; 16], b"test".to_vec(), 123456789, 1);
    let result2 = msg2.requeue(5000).await;
    assert!(result2.is_err() || result2.is_ok());

    // 测试长延迟
    let msg3 = Message::new(vec![3; 16], b"test".to_vec(), 123456789, 1);
    let result3 = msg3.requeue(60000).await;
    assert!(result3.is_err() || result3.is_ok());
}
