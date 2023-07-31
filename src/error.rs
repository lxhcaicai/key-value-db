use std::io;
use failure::Fail;

///  Error type for kvs
#[derive(Fail, Debug)]
pub enum KvsError {

    /// IO error
    #[fail(display = "{}", _0)]
    Io(#[cause] io::Error),

    /// 序列化或反序列化错误
    #[fail(display = "{}", _0)]
    Serde(#[cause] serde_json::Error),

    /// 删除 不存在的键 错误
    #[fail(display = "Key not found")]
    KeyNotFound,

    /// 意外的命令类型错误
    /// 有损坏的日志或程序错误
    #[fail(display = "Unexpected command type")]
    UnexpectedCommandType,
}

impl From<io::Error> for KvsError {
    fn from(err: io::Error) -> Self {
        KvsError::Io(err)
    }
}

impl From<serde_json::Error> for KvsError {
    fn from(err: serde_json::Error) -> Self {
        KvsError::Serde(err)
    }
}

/// Result type for kvs
pub type Result<T> = std::result::Result<T,KvsError>;