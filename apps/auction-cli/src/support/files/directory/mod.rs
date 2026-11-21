//! 以目录描述符固定的 owner-only 私有目录。
//! 子项操作由 `private_directory` 实现，路径与目录项校验由 `path_helpers` 实现。

mod path_helpers;
mod private_directory;
#[cfg(test)]
mod tests;

pub(crate) use private_directory::{FileIdentity, MAX_READ_BYTES, PrivateDirectory};
