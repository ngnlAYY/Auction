//! 应用端口共用的异步返回类型。

use std::future::Future;
use std::pin::Pin;

use crate::errors::ApplicationError;

/// 应用端口与用例的异步返回值：装箱以便 HTTP state 与 Worker 持有动态端口而不暴露具体存储
/// 类型；错误统一为 [`ApplicationError`]。
pub type ApplicationFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ApplicationError>> + Send + 'a>>;
