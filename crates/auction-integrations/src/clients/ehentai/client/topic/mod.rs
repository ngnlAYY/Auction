//! E-Hentai 主题表单、精确标题查找与发布回执解析。
//!
//! 表单加载（`form`）、写入提交（`submit`）、回执与引用解析（`proof`）及精确标题
//! 查找（`lookup`）分置子模块；确认页结构与引用解析只有一处实现，创建与固定帖写入
//! 共用同一套证明门禁。

use self::parent::*;
use crate::clients::ehentai::client as parent;

mod form;
mod lookup;
mod proof;
mod submit;

#[cfg(test)]
mod tests;
