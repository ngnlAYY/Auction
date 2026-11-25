//! 页面挂载加载失败的原因；任何一种都让进程启动失败，不降级回内嵌页面。

use std::io;
use std::path::PathBuf;

/// 加载并校验静态挂载时的失败。文本只含路径与字段名，不含文件内容。
#[derive(Debug, thiserror::Error)]
pub enum WebMountError {
    #[error("读取挂载文件失败：{path}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("入口指针 ENTRY.v1 无效：{0}")]
    Entry(&'static str),
    #[error("清单 MANIFEST.v1 无效：{0}")]
    Manifest(&'static str),
    #[error("挂载路径不安全：{0}")]
    UnsafePath(String),
    #[error("清单登记的文件缺失：{0}")]
    FileMissing(String),
    #[error("挂载目录存在清单外的文件：{0}")]
    FileUnlisted(String),
    #[error("文件摘要与清单不符：{0}")]
    DigestMismatch(String),
    #[error("页面脚本策略与来源不符：{0}")]
    PolicyMismatch(&'static str),
    #[error("两个 build 在同一路径上字节不同：{0}")]
    Conflict(String),
    #[error("挂载内容超出上限：{0}")]
    TooLarge(&'static str),
}
