use std::net::ToSocketAddrs;
use std::sync::Arc;

use log::info;

use crate::connection::Connection;
use crate::error::{Error, Result};
use crate::protocol::{Command, Frame, IdentifyConfig, ProtocolError};

/// 发现NSQ服务器
pub async fn lookup_nsqd_nodes<A: ToSocketAddrs + std::fmt::Display>(
    nsqlookupd_addr: A,
    topic: &str,
) -> Result<Vec<String>> {
    // 这是一个简化版实现，真实场景中应当使用HTTP API查询nsqlookupd
    // 未来版本可实现对nsqlookupd HTTP API的完整支持

    info!(
        "正在查询 {} 上的nsqlookupd以查找主题 {} 的生产者",
        nsqlookupd_addr, topic
    );

    // 实际应用中，这里应该向nsqlookupd发送HTTP请求
    // 例如：GET /lookup?topic={topic}
    // 并解析响应以获取nsqd节点列表

    // 作为简化示例，这里假设我们直接使用提供的nsqlookupd地址作为nsqd节点
    Ok(vec![nsqlookupd_addr.to_string()])
}

/// 创建到NSQ服务器的连接
pub async fn create_nsqd_connection<A: ToSocketAddrs + std::fmt::Display>(
    nsqd_addr: A,
    identify_config: Option<IdentifyConfig>,
    auth_secret: Option<String>,
) -> Result<Arc<Connection>> {
    let connection = Connection::new(
        nsqd_addr,
        identify_config,
        auth_secret,
        std::time::Duration::from_secs(60),  // 默认读超时
        std::time::Duration::from_secs(1),   // 默认写超时
    ).await?;

    Ok(Arc::new(connection))
}

/// 向NSQ主题发布消息
pub async fn publish_message<A: ToSocketAddrs + std::fmt::Display>(
    nsqd_addr: A,
    topic: &str,
    message: Vec<u8>,
) -> Result<()> {
    let connection = create_nsqd_connection(nsqd_addr, None, None).await?;

    info!("向主题 {} 发布消息", topic);

    connection
        .send_command(Command::Publish(topic.to_string(), message))
        .await?;

    // 读取响应以确认发布成功
    match connection.read_frame().await? {
        Frame::Response(_) => Ok(()),
        Frame::Error(data) => {
            let error_str = String::from_utf8_lossy(&data);
            Err(Error::Protocol(ProtocolError::Other(format!(
                "发布消息时出错: {}",
                error_str
            ))))
        }
        _ => Err(Error::Protocol(ProtocolError::Other(
            "发布消息时收到意外响应".to_string(),
        ))),
    }
}

/// 批量向NSQ主题发布消息
pub async fn mpublish_messages<A: ToSocketAddrs + std::fmt::Display>(
    nsqd_addr: A,
    topic: &str,
    messages: Vec<Vec<u8>>,
) -> Result<()> {
    if messages.is_empty() {
        return Ok(());
    }

    let connection = create_nsqd_connection(nsqd_addr, None, None).await?;

    info!("向主题 {} 批量发布 {} 条消息", topic, messages.len());

    connection
        .send_command(Command::Mpublish(topic.to_string(), messages))
        .await?;

    // 读取响应以确认发布成功
    match connection.read_frame().await? {
        Frame::Response(_) => Ok(()),
        Frame::Error(data) => {
            let error_str = String::from_utf8_lossy(&data);
            Err(Error::Protocol(ProtocolError::Other(format!(
                "批量发布消息时出错: {}",
                error_str
            ))))
        }
        _ => Err(Error::Protocol(ProtocolError::Other(
            "批量发布消息时收到意外响应".to_string(),
        ))),
    }
}

/// 从NSQ主题订阅消息
pub async fn subscribe<A: ToSocketAddrs + std::fmt::Display>(
    nsqd_addr: A,
    topic: &str,
    channel: &str,
    ready_count: u32,
) -> Result<Arc<Connection>> {
    let connection = create_nsqd_connection(nsqd_addr, None, None).await?;

    info!("订阅主题 {}, 频道 {}", topic, channel);

    // 发送订阅命令
    connection
        .send_command(Command::Subscribe(topic.to_string(), channel.to_string()))
        .await?;

    // 读取订阅响应
    match connection.read_frame().await? {
        Frame::Response(_) => {
            info!("成功订阅");

            // 设置初始RDY计数
            connection.send_command(Command::Ready(ready_count)).await?;

            Ok(connection)
        }
        Frame::Error(data) => {
            let error_str = String::from_utf8_lossy(&data);
            Err(Error::Protocol(ProtocolError::Other(format!(
                "订阅时出错: {}",
                error_str
            ))))
        }
        _ => Err(Error::Protocol(ProtocolError::Other(
            "订阅时收到意外响应".to_string(),
        ))),
    }
}
