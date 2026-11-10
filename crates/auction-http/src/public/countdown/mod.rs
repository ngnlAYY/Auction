//! 自托管论坛倒计时图片；HTTP 边界、SVG 视图及有界动画各自分离。

mod animation;
mod handler;
mod svg;

pub(crate) use handler::{apply_security_headers, get};
