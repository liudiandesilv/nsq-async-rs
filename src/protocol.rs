use byteorder::{BigEndian, ByteOrder, ReadBytesExt, WriteBytesExt};
use serde::{Deserialize, Serialize};
use std::io::{Cursor, Read};
use thiserror::Error;

use crate::error::{Error, Result};

/// NSQ协议常量
pub const MAGIC_V2: &[u8] = b"  V2";
pub const HEARTBEAT: &[u8] = b"_heartbeat_";
pub const OK: &[u8] = b"OK";
pub const FRAME_TYPE_RESPONSE: i32 = 0;
pub const FRAME_TYPE_ERROR: i32 = 1;
pub const FRAME_TYPE_MESSAGE: i32 = 2;

/// 命令类型枚举
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// 标识服务器身份和特性
    Identify(IdentifyConfig),
    /// 订阅主题和频道
    Subscribe(String, String),
    /// 发布消息到主题
    Publish(String, Vec<u8>),
    /// 延迟发布消息到主题
    DelayedPublish(String, Vec<u8>, u32),
    /// 批量发布消息到主题
    Mpublish(String, Vec<Vec<u8>>),
    /// 准备接收更多消息
    Ready(u32),
    /// 完成处理消息
    Finish(String),
    /// 重新入队消息
    Requeue(String, u32),
    /// 标记消息需要延迟处理
    Touch(String),
    /// 处理不同的响应类型
    Nop,
    /// 清理和关闭连接
    Cls,
    /// 认证
    Auth(Option<String>),
}

impl Command {
    /// 将命令转换为字节以便发送
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut buf = Vec::new();
        match self {
            Command::Identify(config) => {
                buf.extend_from_slice(b"IDENTIFY\n");
                let json = serde_json::to_string(config)?;
                buf.write_u32::<BigEndian>(json.len() as u32)?;
                buf.extend_from_slice(json.as_bytes());
            }
            Command::Subscribe(topic, channel) => {
                let cmd = format!("SUB {} {}\n", topic, channel);
                buf.extend_from_slice(cmd.as_bytes());
            }
            Command::Publish(topic, body) => {
                let cmd = format!("PUB {}\n", topic);
                buf.extend_from_slice(cmd.as_bytes());
                buf.write_u32::<BigEndian>(body.len() as u32)?;
                buf.extend_from_slice(body.as_slice());
            }
            Command::DelayedPublish(topic, body, delay) => {
                let cmd = format!("DPUB {} {}\n", topic, delay);
                buf.extend_from_slice(cmd.as_bytes());
                buf.write_u32::<BigEndian>(body.len() as u32)?;
                buf.extend_from_slice(body.as_slice());
            }
            Command::Mpublish(topic, bodies) => {
                let cmd = format!("MPUB {}\n", topic);
                buf.extend_from_slice(cmd.as_bytes());

                // 计算总大小: 4字节(消息数量) + 每个消息的(4字节大小 + 内容)
                let mut total_size = 4;
                for body in bodies {
                    total_size += 4 + body.len();
                }

                buf.write_u32::<BigEndian>(total_size as u32)?;
                buf.write_u32::<BigEndian>(bodies.len() as u32)?;

                for body in bodies {
                    buf.write_u32::<BigEndian>(body.len() as u32)?;
                    buf.extend_from_slice(body);
                }
            }
            Command::Ready(count) => {
                let cmd = format!("RDY {}\n", count);
                buf.extend_from_slice(cmd.as_bytes());
            }
            Command::Finish(id) => {
                let cmd = format!("FIN {}\n", id);
                buf.extend_from_slice(cmd.as_bytes());
            }
            Command::Requeue(id, delay) => {
                let cmd = format!("REQ {} {}\n", id, delay);
                buf.extend_from_slice(cmd.as_bytes());
            }
            Command::Touch(id) => {
                let cmd = format!("TOUCH {}\n", id);
                buf.extend_from_slice(cmd.as_bytes());
            }
            Command::Nop => {
                buf.extend_from_slice(b"NOP\n");
            }
            Command::Cls => {
                buf.extend_from_slice(b"CLS\n");
            }
            Command::Auth(secret) => {
                buf.extend_from_slice(b"AUTH\n");
                if let Some(s) = secret {
                    buf.write_u32::<BigEndian>(s.len() as u32)?;
                    buf.extend_from_slice(s.as_bytes());
                } else {
                    buf.write_u32::<BigEndian>(0)?;
                }
            }
        }

        Ok(buf)
    }
}

/// NSQ消息格式
#[derive(Debug, Clone)]
pub struct Message {
    /// 唯一消息ID
    pub id: Vec<u8>,
    /// 消息时间戳
    pub timestamp: u64,
    /// 消息尝试次数
    pub attempts: u16,
    /// 消息体
    pub body: Vec<u8>,
}

impl Message {
    /// 从字节流解析消息
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 26 {
            return Err(Error::Protocol(ProtocolError::Other(
                "消息大小不足".to_string(),
            )));
        }

        let mut cursor = Cursor::new(bytes);

        // 跳过消息大小
        cursor.set_position(4);

        // 消息帧类型，2表示消息
        let frame_type = cursor.read_u32::<BigEndian>()?;
        if frame_type != 2 {
            return Err(Error::Protocol(ProtocolError::Other(format!(
                "无效的帧类型: {}",
                frame_type
            ))));
        }

        // 读取时间戳 (8字节)
        let timestamp = cursor.read_u64::<BigEndian>()?;

        // 读取尝试次数 (2字节)
        let attempts = cursor.read_u16::<BigEndian>()?;

        // 读取消息ID (16字节)
        let mut id_bytes = [0u8; 16];
        cursor.read_exact(&mut id_bytes)?;
        let id = id_bytes.to_vec();

        // 读取消息体
        let mut body = Vec::new();
        cursor.read_to_end(&mut body)?;

        Ok(Self {
            id,
            timestamp,
            attempts,
            body,
        })
    }
}

/// IDENTIFY命令的配置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IdentifyConfig {
    /// 客户端标识，默认为hostname
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,

    /// 客户端主机名
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,

    /// 客户端功能特性
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feature_negotiation: Option<bool>,

    /// 心跳间隔(毫秒)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heartbeat_interval: Option<i32>,

    /// 输出缓冲大小
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_buffer_size: Option<i32>,

    /// 输出缓冲超时(毫秒)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_buffer_timeout: Option<i32>,

    /// TLS设置
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_v1: Option<bool>,

    /// 压缩设置
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snappy: Option<bool>,

    /// 延迟采样率
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<i32>,

    /// 用户代理
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,

    /// 消息超时(毫秒)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub msg_timeout: Option<i32>,
}

impl Default for IdentifyConfig {
    fn default() -> Self {
        let hostname = hostname::get()
            .ok()
            .and_then(|h| h.into_string().ok())
            .unwrap_or_else(|| "unknown".to_string());

        Self {
            client_id: Some(hostname.clone()),
            hostname: Some(hostname),
            feature_negotiation: Some(true),
            heartbeat_interval: Some(30000),
            output_buffer_size: Some(16384),
            output_buffer_timeout: Some(250),
            tls_v1: None,
            snappy: None,
            sample_rate: None,
            user_agent: Some(format!("rust-nsq/{}", env!("CARGO_PKG_VERSION"))),
            msg_timeout: Some(60000),
        }
    }
}

/// 帧类型
#[derive(Debug, Clone, PartialEq)]
pub enum FrameType {
    /// 响应
    Response,
    /// 错误
    Error,
    /// 消息
    Message,
}

impl TryFrom<u32> for FrameType {
    type Error = Error;

    fn try_from(value: u32) -> Result<Self> {
        match value {
            0 => Ok(FrameType::Response),
            1 => Ok(FrameType::Error),
            2 => Ok(FrameType::Message),
            _ => Err(Error::Protocol(ProtocolError::Other(format!(
                "未知帧类型: {}",
                value
            )))),
        }
    }
}

/// 读取NSQ帧
pub fn read_frame(data: &[u8]) -> Result<(FrameType, &[u8])> {
    if data.len() < 8 {
        return Err(Error::Protocol(ProtocolError::Other(
            "帧数据不完整".to_string(),
        )));
    }

    let mut cursor = Cursor::new(data);
    let size = cursor.read_u32::<BigEndian>()?;

    if data.len() < (size as usize + 4) {
        return Err(Error::Protocol(ProtocolError::Other(
            "帧数据不完整".to_string(),
        )));
    }

    let frame_type_raw = cursor.read_u32::<BigEndian>()?;
    let frame_type = FrameType::try_from(frame_type_raw)?;

    // 返回帧类型和帧数据(不包括大小和类型)
    Ok((frame_type, &data[8..(size as usize + 4)]))
}

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid frame size")]
    InvalidFrameSize,
    #[error("Invalid magic version")]
    InvalidMagicVersion,
    #[error("Invalid frame type: {0}")]
    InvalidFrameType(i32),
    #[error("Protocol error: {0}")]
    Other(String),
}

#[derive(Debug)]
pub enum Frame {
    Response(Vec<u8>),
    Error(Vec<u8>),
    Message(Message),
}

pub struct Protocol;

impl Protocol {
    pub fn write_command(cmd: &[u8], params: &[&[u8]]) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(cmd);

        if !params.is_empty() {
            buf.push(b' ');
            for (i, param) in params.iter().enumerate() {
                if i > 0 {
                    buf.push(b' ');
                }
                buf.extend_from_slice(param);
            }
        }

        buf.extend_from_slice(b"\n");
        buf
    }

    pub fn decode_message(data: &[u8]) -> Result<Message> {
        if data.len() < 4 {
            return Err(Error::Protocol(ProtocolError::InvalidFrameSize));
        }

        let timestamp = BigEndian::read_u64(&data[0..8]);
        let attempts = BigEndian::read_u16(&data[8..10]);
        let id = data[10..26].to_vec();
        let body = data[26..].to_vec();

        Ok(Message {
            timestamp,
            attempts,
            id,
            body,
        })
    }

    pub fn decode_frame(data: &[u8]) -> Result<Frame> {
        if data.len() < 4 {
            return Err(Error::Protocol(ProtocolError::InvalidFrameSize));
        }

        let frame_type = BigEndian::read_i32(&data[0..4]);
        let frame_data = &data[4..];

        match frame_type {
            FRAME_TYPE_RESPONSE => Ok(Frame::Response(frame_data.to_vec())),
            FRAME_TYPE_ERROR => Ok(Frame::Error(frame_data.to_vec())),
            FRAME_TYPE_MESSAGE => {
                let msg = Self::decode_message(frame_data)?;
                Ok(Frame::Message(msg))
            }
            _ => Err(Error::Protocol(ProtocolError::InvalidFrameType(frame_type))),
        }
    }

    pub fn encode_command(name: &str, body: Option<&[u8]>, params: &[&str]) -> Vec<u8> {
        let mut cmd = Vec::new();

        // Write size placeholder
        cmd.extend_from_slice(&[0; 4]);

        // Write command name
        cmd.extend_from_slice(name.as_bytes());

        // Write parameters
        for param in params {
            cmd.push(b' ');
            cmd.extend_from_slice(param.as_bytes());
        }

        cmd.push(b'\n');

        // Write body if present
        if let Some(body) = body {
            cmd.extend_from_slice(body);
        }

        // Update size
        let size = (cmd.len() - 4) as u32;
        let mut size_bytes = [0; 4];
        BigEndian::write_u32(&mut size_bytes, size);
        cmd[0..4].copy_from_slice(&size_bytes);

        cmd
    }
}

// Common NSQ commands
pub const IDENTIFY: &str = "IDENTIFY";
pub const SUB: &str = "SUB";
pub const PUB: &str = "PUB";
pub const MPUB: &str = "MPUB";
pub const RDY: &str = "RDY";
pub const FIN: &str = "FIN";
pub const REQ: &str = "REQ";
pub const TOUCH: &str = "TOUCH";
pub const CLS: &str = "CLS";
pub const NOP: &str = "NOP";
pub const AUTH: &str = "AUTH";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identify_command() {
        let config = IdentifyConfig {
            client_id: Some("test_client".to_string()),
            hostname: Some("test_host".to_string()),
            feature_negotiation: Some(true),
            ..Default::default()
        };

        let cmd = Command::Identify(config);
        let bytes = cmd.to_bytes().unwrap();

        // 验证命令前缀
        assert!(bytes.starts_with(b"IDENTIFY\n"));
    }

    #[test]
    fn test_publish_command() {
        let topic = "test_topic".to_string();
        let message = b"test message".to_vec();

        let cmd = Command::Publish(topic, message.clone());
        let bytes = cmd.to_bytes().unwrap();

        // 验证命令前缀
        assert!(bytes.starts_with(b"PUB test_topic\n"));

        // 验证消息内容
        let message_size_bytes = &bytes[15..19];
        let mut cursor = Cursor::new(message_size_bytes);
        let message_size = cursor.read_u32::<BigEndian>().unwrap();
        assert_eq!(message_size as usize, message.len());

        let actual_message = &bytes[19..];
        assert_eq!(actual_message, message.as_slice());
    }
}
