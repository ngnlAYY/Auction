//! 从 CLI 二进制入口验证密钥轮换参数与退出码。

use std::process::Command;

#[cfg(not(all(feature = "consignment-dev-preview", debug_assertions)))]
#[test]
fn consignment_config_check_and_keys_reject_before_database() -> std::io::Result<()> {
    use std::fs;
    use std::os::unix::fs::DirBuilderExt;

    use auction_config::ConfigError;
    use uuid::Uuid;

    let directory = std::env::temp_dir().join(format!("auction-cli-keys-{}", Uuid::new_v4()));
    fs::DirBuilder::new().mode(0o700).create(&directory)?;
    let outputs = (|| -> std::io::Result<_> {
        fs::write(directory.join("common.toml"), "[consignment]\nenabled = true\n")?;
        // 无数据库地址或装备钥，命令即使绕过制品门也不会连接数据库。
        fs::write(directory.join("secrets.toml"), "")?;
        for role in ["public", "admin", "worker"] {
            fs::write(
                directory.join(format!("{role}.toml")),
                "common_file = 'common.toml'\nsecrets_file = 'secrets.toml'\n",
            )?;
        }
        [
            (&["config", "check"][..], 2),
            (&["keys", "status"][..], 5),
            (&["keys", "reencrypt", "--domain", "equipment"][..], 5),
        ]
        .into_iter()
        .map(|(args, exit_code)| {
            let output = Command::new(env!("CARGO_BIN_EXE_auction-cli"))
                .arg("--config-dir")
                .arg(&directory)
                .args(args)
                .output()?;
            Ok((args, exit_code, output))
        })
        .collect::<std::io::Result<Vec<_>>>()
    })();
    fs::remove_dir_all(directory)?;

    let reason = ConfigError::ConsignmentPreviewRequired.to_string();
    for (args, exit_code, output) in outputs? {
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(exit_code), "{args:?}: {stderr}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert!(stderr.contains(&reason), "{args:?}: {stderr}");
    }
    Ok(())
}

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
