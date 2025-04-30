// Rust NSQ客户端库
// 导出所有公共模块

// 导出模块
pub mod commands;
pub mod connection;
pub mod consumer;
pub mod error;
pub mod lookup;
pub mod producer;
pub mod protocol;

// 重导出常用类型
pub use consumer::{Consumer, ConsumerConfig, Handler};
pub use error::{Error, Result};
pub use producer::{new_producer, Producer, ProducerConfig};

pub use connection::Connection;
pub use protocol::{Frame, Protocol, ProtocolError};
