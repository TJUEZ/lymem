//! 统一错误类型。

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("数据库错误: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("全文索引错误: {0}")]
    Fts(#[from] tantivy::TantivyError),
    #[error("嵌入错误: {0}")]
    Embed(String),
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("非法输入: {0}")]
    InvalidInput(String),
    #[error("未找到: {0}")]
    NotFound(String),
}

pub type Result<T> = std::result::Result<T, CoreError>;
