//! 页面响应级脚本策略：由页面分派按所选 build 写入，安全头中间件优先采用。

use super::WebScriptPolicy;

/// 响应扩展：这个页面响应所属 build 的脚本策略。
///
/// 不变量：策略与页面字节同源绑定（`PageBuild` 构造后不可拆开），中间件只读取、不推断。
/// 没有该扩展的响应（API、错误页、重定向）使用 surface 默认策略，因此旧壳的放宽策略
/// 不会渗入任何非页面响应。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PageScriptPolicy(pub(crate) WebScriptPolicy);
