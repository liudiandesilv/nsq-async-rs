use env_logger;
use log::info;
use nsq_async_rs::producer::{NsqProducer, Producer, ProducerConfig};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

#[tokio::test]
async fn test_connection_reuse() {
    // 设置日志
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Info)
        .format_timestamp_millis()
        .init();

    let config = ProducerConfig {
        nsqd_addresses: vec!["127.0.0.1:4150".to_string()],
        connection_timeout: Duration::from_secs(5),
        ..Default::default()
    };

    let producer = NsqProducer::new(config);

    // 第一次发布消息
    info!("第一次发布消息");
    let result1 = producer.publish("test_topic", "message1").await;
    assert!(result1.is_ok());

    // 获取当前连接池中的连接数
    let pool_size = producer.get_connection_pool_size().await;
    info!("当前连接池大小: {}", pool_size);

    // 第二次发布消息，应该复用连接
    info!("第二次发布消息");
    let result2 = producer.publish("test_topic", "message2").await;
    assert!(result2.is_ok());

    // 验证连接池大小是否保持不变
    let new_pool_size = producer.get_connection_pool_size().await;
    info!("发布后连接池大小: {}", new_pool_size);
    assert_eq!(pool_size, new_pool_size, "连接池大小应该保持不变");

    // 测试并发发布
    info!("开始并发发布测试");
    let producer = Arc::new(producer);
    let mut handles = vec![];

    for i in 0..5 {
        let producer = producer.clone();
        handles.push(tokio::spawn(async move {
            producer
                .publish("test_topic", format!("concurrent_message{}", i))
                .await
                .unwrap()
        }));
    }

    for handle in handles {
        assert!(handle.await.is_ok());
    }

    // 验证并发发布后的连接池大小
    let final_pool_size = producer.get_connection_pool_size().await;
    info!("并发发布后连接池大小: {}", final_pool_size);
    assert_eq!(
        pool_size, final_pool_size,
        "并发发布后连接池大小应该保持不变"
    );
}

#[tokio::test]
async fn test_connection_recovery() {
    let config = ProducerConfig {
        nsqd_addresses: vec!["127.0.0.1:4150".to_string()],
        connection_timeout: Duration::from_secs(5),
        ..Default::default()
    };

    let producer = NsqProducer::new(config);

    // 第一次发布消息
    let result1 = producer.publish("test_topic", "message1").await;
    assert!(result1.is_ok());

    // 等待一小段时间
    sleep(Duration::from_millis(100)).await;

    // 再次发布消息，应该自动重连
    let result2 = producer.publish("test_topic", "message2").await;
    assert!(result2.is_ok());
}

#[tokio::test]
async fn test_concurrent_publish() {
    let config = ProducerConfig {
        nsqd_addresses: vec!["127.0.0.1:4150".to_string()],
        connection_timeout: Duration::from_secs(5),
        ..Default::default()
    };

    let producer = Arc::new(NsqProducer::new(config));
    let mut handles = vec![];

    // 创建多个并发任务发布消息
    for i in 0..10 {
        let producer = producer.clone();
        handles.push(tokio::spawn(async move {
            producer
                .publish("test_topic", format!("message{}", i))
                .await
                .unwrap()
        }));
    }

    // 等待所有任务完成
    for handle in handles {
        assert!(handle.await.is_ok());
    }
}

#[tokio::test]
async fn test_multiple_topics() {
    let config = ProducerConfig {
        nsqd_addresses: vec!["127.0.0.1:4150".to_string()],
        connection_timeout: Duration::from_secs(5),
        ..Default::default()
    };

    let producer = NsqProducer::new(config);

    // 向不同主题发布消息
    let result1 = producer.publish("topic1", "message1").await;
    assert!(result1.is_ok());

    let result2 = producer.publish("topic2", "message2").await;
    assert!(result2.is_ok());
}

#[tokio::test]
async fn test_delayed_publish() {
    let config = ProducerConfig {
        nsqd_addresses: vec!["127.0.0.1:4150".to_string()],
        connection_timeout: Duration::from_secs(5),
        ..Default::default()
    };

    let producer = NsqProducer::new(config);

    // 发布延迟消息
    let result = producer
        .publish_delayed("test_topic", "delayed_message", Duration::from_secs(1))
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_batch_publish() {
    let config = ProducerConfig {
        nsqd_addresses: vec!["127.0.0.1:4150".to_string()],
        connection_timeout: Duration::from_secs(5),
        ..Default::default()
    };

    let producer = NsqProducer::new(config);

    // 准备批量消息
    let messages = vec!["message1", "message2", "message3"];

    // 批量发布消息
    let result = producer.publish_multi("test_topic", messages).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_ping() {
    let config = ProducerConfig {
        nsqd_addresses: vec!["127.0.0.1:4150".to_string()],
        connection_timeout: Duration::from_secs(5),
        ..Default::default()
    };

    let producer = NsqProducer::new(config);

    // 测试 ping 功能
    let result = producer.ping(None, None).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_invalid_address() {
    let config = ProducerConfig {
        nsqd_addresses: vec!["invalid_address:4150".to_string()],
        connection_timeout: Duration::from_secs(1),
        ..Default::default()
    };

    let producer = NsqProducer::new(config);

    // 测试无效地址
    let result = producer.publish("test_topic", "message").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_connection_with_interval() {
    // 设置日志
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Info)
        .format_timestamp_millis()
        .init();

    let config = ProducerConfig {
        nsqd_addresses: vec!["127.0.0.1:4150".to_string()],
        connection_timeout: Duration::from_secs(5),
        ..Default::default()
    };

    let producer = Arc::new(NsqProducer::new(config));

    // 记录初始连接池大小
    let initial_pool_size = producer.get_connection_pool_size().await;
    info!("初始连接池大小: {}", initial_pool_size);

    // 创建定时器
    let mut interval = tokio::time::interval(Duration::from_secs(2));
    let mut total_messages = 0;
    let mut success_messages = 0;
    let mut failed_messages = 0;
    let mut connection_drops = 0;
    let mut max_pool_size = initial_pool_size;
    let mut min_pool_size = initial_pool_size;
    let start_time = std::time::Instant::now();

    // 主循环
    loop {
        interval.tick().await;

        // 检查连接池大小变化
        let current_pool_size = producer.get_connection_pool_size().await;
        if current_pool_size < min_pool_size {
            min_pool_size = current_pool_size;
            connection_drops += 1;
            info!("警告：连接池大小减少，当前大小: {}", current_pool_size);
        }
        if current_pool_size > max_pool_size {
            max_pool_size = current_pool_size;
            info!("连接池大小增加，当前大小: {}", current_pool_size);
        }

        // 创建多个并发发送任务
        let mut handles = vec![];
        for i in 0..3 {
            let producer = producer.clone();
            handles.push(tokio::spawn(async move {
                let msg = format!("message_{}_{}", i, total_messages);
                match producer.publish("test_topic", &msg).await {
                    Ok(_) => {
                        info!("消息发送成功: {}", msg);
                        true
                    }
                    Err(e) => {
                        info!("消息发送失败: {} - {:?}", msg, e);
                        false
                    }
                }
            }));
        }

        // 等待所有任务完成并统计结果
        for handle in handles {
            if handle.await.unwrap() {
                success_messages += 1;
            } else {
                failed_messages += 1;
            }
            total_messages += 1;
        }

        // 输出统计信息
        let elapsed = start_time.elapsed();
        info!(
            "运行时间: {:?}, 总消息数: {}, 成功: {}, 失败: {}, 成功率: {:.2}%, 连接丢失次数: {}, 当前连接池大小: {}, 最小连接池大小: {}, 最大连接池大小: {}",
            elapsed,
            total_messages,
            success_messages,
            failed_messages,
            (success_messages as f64 / total_messages as f64) * 100.0,
            connection_drops,
            current_pool_size,
            min_pool_size,
            max_pool_size
        );

        // 如果运行时间超过2分钟，结束测试
        if elapsed.as_secs() >= 120 {
            break;
        }
    }

    // 最终检查连接池大小
    let final_pool_size = producer.get_connection_pool_size().await;
    info!("最终连接池大小: {}", final_pool_size);

    // 输出最终统计信息
    info!(
        "测试总结 - 总运行时间: {:?}, 总消息数: {}, 成功: {}, 失败: {}, 成功率: {:.2}%, 连接丢失次数: {}, 最小连接池大小: {}, 最大连接池大小: {}, 最终连接池大小: {}",
        start_time.elapsed(),
        total_messages,
        success_messages,
        failed_messages,
        (success_messages as f64 / total_messages as f64) * 100.0,
        connection_drops,
        min_pool_size,
        max_pool_size,
        final_pool_size
    );

    // 额外等待一段时间，确保连接仍然可用
    sleep(Duration::from_secs(3)).await;

    // 发送最后一条消息验证连接可用性
    let result = producer.publish("test_topic", "final_test_message").await;
    assert!(result.is_ok(), "连接应该仍然可用");

    // 再次检查连接池大小
    let very_final_pool_size = producer.get_connection_pool_size().await;
    info!("最终验证连接池大小: {}", very_final_pool_size);
    assert_eq!(
        initial_pool_size, very_final_pool_size,
        "经过所有测试后连接池大小应该保持稳定"
    );
}
