//! 寄售派发用例：规范摘要、系统命令 UUIDv5、起写许可的唯一铸造点与端口命令组装。
//!
//! 保护不变量 4、5：POST 许可只由本服务在收到存储的“首次提交”结果后创建并以值移动一次，
//! 不从账本、回执或新 fence 重建；重放、新 fence、提交未知都只能得到“已提交”。

mod authorize;
mod certify;
mod encoding;
mod evidence;
mod handoff;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use crate::ports::consignment::DispatchStore;

/// 派发写用例；只依赖窄端口。
pub struct DispatchService {
    store: Arc<dyn DispatchStore>,
}

impl DispatchService {
    /// 注入派发写端口。
    pub fn new(store: Arc<dyn DispatchStore>) -> Self {
        Self { store }
    }
}

pub use certify::certify_cod_command;
pub use evidence::RecordEvidenceInput;
pub use handoff::ProveHandoffInput;
