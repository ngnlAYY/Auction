//! 从 CLI 二进制入口验证密钥轮换参数与退出码。

use std::process::Command;

#[test]
fn keys_status_rejects_conflicting_modes_at_command_entry() -> std::io::Result<()> {
    let output = Command::new(env!("CARGO_BIN_EXE_auction-cli"))
        .args(["keys", "status", "--strict", "--require-current-equipment"])
        .output()?;
    assert_eq!(output.status.code(), Some(2));
    let reason = String::from_utf8_lossy(&output.stderr);
    assert!(reason.contains("--strict") && reason.contains("--require-current-equipment"));
    assert!(output.stdout.is_empty());
    Ok(())
}
