//! 旧页面字节的有界取回客户端：只面向回环源站，逐文件 GET，禁重定向、禁代理、禁自动解压。
//!
//! 调用方（CLI 的静态物化）负责清单与摘要核对；这里只保证“拿到的就是源站按所请求表示发出的
//! 原始实体字节”，并把越界响应变成错误而不是截断或降级。

mod client;
mod policy;
#[cfg(test)]
mod tests;

pub use client::{WebFetchClient, WebFetchError, WebFetched};
pub use policy::{WebFetchPolicy, WebRepresentation};
