//! 命令共用的文件安全与输入校验工具；不承载业务规则。

pub(crate) mod files;
#[cfg(feature = "postgres")]
pub(crate) mod text;
