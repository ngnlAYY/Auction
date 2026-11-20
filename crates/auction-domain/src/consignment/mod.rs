//! 一口价挂单的领域模型、资格、状态迁移与定价的公共入口。

mod eligibility;
mod errors;
mod identity;
mod listing;
mod pricing;
mod transition;

#[allow(unused_imports, reason = "历史资格模块由后续任务填充")]
pub use eligibility::*;
pub use errors::*;
#[allow(unused_imports, reason = "装备身份模块由后续任务填充")]
pub use identity::*;
#[allow(unused_imports, reason = "挂单模型模块由后续任务填充")]
pub use listing::*;
#[allow(unused_imports, reason = "定价模块由后续任务填充")]
pub use pricing::*;
#[allow(unused_imports, reason = "状态迁移模块由后续任务填充")]
pub use transition::*;
