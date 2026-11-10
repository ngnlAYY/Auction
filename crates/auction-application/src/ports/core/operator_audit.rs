//! CLI 操作者的主机身份；只用于追加审计，不改变业务命令或幂等摘要。

use serde_json::Value;

/// 由 CLI 装配根采集的有效系统 UID 与可选 sudo 发起者；不代表额外认证或授权。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliOperatorIdentity {
    pub effective_uid: u32,
    pub sudo_user: Option<String>,
}

/// 只为显式来自 CLI 的写入附加身份；Admin 与 Worker 调用保持原 metadata。
pub fn cli_audit_metadata(identity: Option<&CliOperatorIdentity>, mut metadata: Value) -> Value {
    if let Some(identity) = identity {
        metadata["entrypoint"] = Value::from("cli");
        metadata["os_uid"] = Value::from(identity.effective_uid);
        if let Some(sudo_user) = &identity.sudo_user {
            metadata["sudo_user"] = Value::from(sudo_user.as_str());
        }
    }
    metadata
}
