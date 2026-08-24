//! 需要从标准输入读取的受限启动凭据输入。

use super::super::*;

pub(crate) fn read_bootstrap_cookie() -> anyhow::Result<String> {
    const MAX_COOKIE_BYTES: usize = 16 * 1024;
    let mut bytes = Vec::with_capacity(4096);
    std::io::stdin().take((MAX_COOKIE_BYTES + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_COOKIE_BYTES {
        anyhow::bail!("bootstrap mail Cookie 超过允许长度");
    }
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    String::from_utf8(bytes).map_err(Into::into)
}
