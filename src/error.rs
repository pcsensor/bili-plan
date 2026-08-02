//! 应用错误类型：与 Python 脚本的错误消息保持对齐。

use std::fmt;

/// 用户输入错误（对应 Python `ValueError`）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub ErrorKind);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    /// 用户输入类错误
    Input(String),
    /// B 站 API 返回错误（含中文提示）
    Api(String),
    /// 网络 / JSON 解析错误
    Network(String),
    /// 数据为空等业务错误
    Data(String),
}

impl Error {
    pub fn input(msg: impl Into<String>) -> Self {
        Self(ErrorKind::Input(msg.into()))
    }
    pub fn api(msg: impl Into<String>) -> Self {
        Self(ErrorKind::Api(msg.into()))
    }
    pub fn network(msg: impl Into<String>) -> Self {
        Self(ErrorKind::Network(msg.into()))
    }
    pub fn data(msg: impl Into<String>) -> Self {
        Self(ErrorKind::Data(msg.into()))
    }

    /// 面向用户的错误消息（与 Python 脚本输出一致）。
    pub fn message(&self) -> &str {
        match &self.0 {
            ErrorKind::Input(m)
            | ErrorKind::Api(m)
            | ErrorKind::Network(m)
            | ErrorKind::Data(m) => m,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message())
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
