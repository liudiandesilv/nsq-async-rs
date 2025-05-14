use log::{error, info, warn};
use nsq_async_rs::{
    producer::{new_producer, NsqProducer},
    Producer, ProducerConfig,
};
use std::error::Error;
use std::time::{Duration, Instant};
use tokio::time::interval;

use nsq_async_rs::pool::{Pool, PoolConfig, PoolError};

// NSQ 连接工厂函数 - 创建新的 NSQ 生产者
fn nsq_producer_factory() -> Result<NsqProducer, PoolError> {
    let p_cfg = ProducerConfig {
        nsqd_addresses: vec!["127.0.0.1:4150".to_string()],
        ..Default::default()
    };

    Ok(new_producer(p_cfg))
}

// NSQ 连接关闭函数
fn nsq_producer_closer(_producer: &NsqProducer) -> Result<(), String> {
    // NSQ 生产者不需要显式关闭，只需删除引用即可
    // 当引用计数为 0 时会自动清理
    Ok(())
}

// NSQ 连接检查函数
fn nsq_producer_pinger(_producer: &NsqProducer) -> Result<(), String> {
    // 注意：ping 是异步方法，但这里需要同步函数
    // 所以不做实际检查，直接返回成功
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // 配置日志
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Info)
        .format_timestamp_millis()
        .init();

    info!("启动连接池 NSQ 生产者测试...");

    // 创建连接池配置
    let pool_config = PoolConfig {
        initial_cap: 3,            // 初始连接数
        max_cap: 10,               // 最大连接数
        max_idle: 5,               // 最大空闲连接
        idle_timeout: Duration::from_secs(30),  // 空闲超时时间
        max_lifetime: Duration::from_secs(15),  // 连接最大生命周期 (15秒)
    };

    info!("[测试配置] 连接生命周期设置为: {:?}", pool_config.max_lifetime);

    // 创建 NSQ 生产者连接池
    let pool = Pool::new(
        pool_config,
        nsq_producer_factory,
        nsq_producer_closer,
        Some(nsq_producer_pinger),
    )
    .await?;

    info!("NSQ 生产者连接池已创建，开始测试...");

    // 测试预热
    let topic = "test_topic";
    let pooled_conn = pool.get().await?;
    let _ = pooled_conn.conn.publish(topic, "warmup").await;
    pool.put(pooled_conn).await?;

    info!("连接池预热完成。开始定时发送消息...");

    // 创建定时器，每2秒发送一个批次，增加间隔以便连接有更多时间停留在池中
    let mut interval = interval(Duration::from_secs(2));

    // 定时发送消息，每个连接发送五次，然后归还
    // 增加批次数到50，确保测试时间足够长以观察连接生命周期管理
    for batch in 1..=50 {
        interval.tick().await; // 等待下一个时间间隔
        let start_batch = Instant::now();

        // 从连接池获取一个连接
        match pool.get().await {
            Ok(pooled_conn) => {
                info!(
                    "批次 #{}: 获得连接，连接创建时间: {:?}，已运行: {:?}",
                    batch,
                    pooled_conn.created_time,
                    pooled_conn.created_time.elapsed()
                );

                // 使用同一个连接发送五条消息
                for j in 1..=5 {
                    let msg_id = (batch - 1) * 5 + j;
                    let start = Instant::now();
                    let topic = "test_topic";
                    let message = format!("test message #{}", msg_id).into_bytes();

                    match pooled_conn.conn.publish(topic, message.clone()).await {
                        Ok(_) => {
                            info!("  消息 #{} 发送成功，耗时: {:?}", msg_id, start.elapsed());
                        }
                        Err(e) => {
                            error!("  消息 #{} 发送失败: {}", msg_id, e);
                        }
                    }

                    // 短暂停细粒度异步
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }

                // 归还连接到连接池
                info!(
                    "批次 #{}: 归还连接，共使用了 {:?}",
                    batch,
                    start_batch.elapsed()
                );

                if let Err(e) = pool.put(pooled_conn).await {
                    error!("归还连接失败: {}", e);
                }
            }
            Err(e) => {
                error!("从连接池获取连接失败: {}", e);
            }
        }

        // 每 5 条消息显示一次连接池状态
        if batch % 5 == 0 {
            info!(
                "连接池状态 - 活跃连接: {}, 空闲连接: {}",
                pool.active_count().await,
                pool.idle_count().await
            );
        }
    }

    // 等待最后一条消息处理完成
    tokio::time::sleep(Duration::from_secs(1)).await;

    // 关闭连接池
    info!("测试完成，关闭连接池...");
    pool.release().await?;

    info!("连接池已关闭，测试结束");
    Ok(())
}
