use std::io;
use thiserror::Error;
use tokio::time::error::Elapsed;

use crate::protocol::ProtocolError;

/// NSQ客户端库错误类型
#[derive(Debug, Error)]
pub enum Error {
    /// IO错误
    #[error("IO错误: {0}")]
    Io(#[from] io::Error),

    /// 序列化错误
    #[error("序列化错误: {0}")]
    Serialize(#[from] serde_json::Error),

    /// 连接错误
    #[error("连接错误: {0}")]
    Connection(String),

    /// 协议错误
    #[error("协议错误: {0}")]
    Protocol(#[from] ProtocolError),

    /// 超时错误
    #[error("操作超时")]
    Timeout(#[from] Elapsed),

    /// 认证错误
    #[error("认证失败: {0}")]
    Auth(String),

    /// 消息处理错误
    #[error("消息处理错误: {0}")]
    MessageHandling(String),

    /// 配置错误
    #[error("配置错误: {0}")]
    Config(String),

    /// HTTP错误
    #[error("HTTP错误: {0}")]
    Http(#[from] reqwest::Error),

    /// 其他错误
    #[error("其他错误: {0}")]
    Other(String),
}

impl From<backoff::Error<Error>> for Error {
    fn from(err: backoff::Error<Error>) -> Self {
        match err {
            backoff::Error::Permanent(e) => e,
            backoff::Error::Transient { err, .. } => err,
        }
    }
}

/// Result类型别名，用于NSQ客户端库
pub type Result<T> = std::result::Result<T, Error>;
