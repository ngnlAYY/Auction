//! stdout 只承载机器可读结果，stderr 承载进度；两者都不得包含连接串、Cookie 或密钥。

use std::io::Write;

use serde::Serialize;

/// 以单个 JSON 文档输出命令结果。
pub(crate) fn print_json<T: Serialize + ?Sized>(value: &T) -> anyhow::Result<()> {
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, value)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}

/// 输出一行人类可读进度。
pub(crate) fn progress(message: impl std::fmt::Display) {
    eprintln!("[auction-cli] {message}");
}
