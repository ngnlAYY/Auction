//! 登录 key 回信的冻结收件人与当前角色绑定校验，供续租与 Admin 命令复用。

use serde_json::Value;
use sqlx::Postgres;
use uuid::Uuid;

use crate::database::TrackedTransaction as Transaction;

fn is_key_reply(origin: &str, template: Option<&str>) -> bool {
    origin == "mail_intake" && matches!(template, Some("register_key" | "rekey_key"))
}

pub(in crate::database::delivery) fn key_reply_user(origin: &str, payload: &Value) -> Option<Uuid> {
    if payload.get("operation")?.as_str()? != "send"
        || !is_key_reply(origin, payload.get("template").and_then(Value::as_str))
    {
        return None;
    }
    Uuid::parse_str(payload.get("user_key_user_id")?.as_str()?).ok().filter(|id| !id.is_nil())
}

/// 按用户 UUID 读取他的当前绑定。写入方持有投递行锁，并在同一事务内锁定用户以排序换绑与发送授权。
pub(in crate::database::delivery) async fn binding_matches(
    transaction: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    recipient: &str,
    lock: bool,
) -> Result<bool, sqlx::Error> {
    // 注册、轮换与换绑都以 username 为邮件身份；旧兼容镜像允许为空，不参与授权。
    let query = if lock {
        "SELECT username FROM public.users WHERE id = $1 FOR SHARE"
    } else {
        "SELECT username FROM public.users WHERE id = $1"
    };
    let current = sqlx::query_scalar::<_, String>(query)
        .bind(user_id)
        .fetch_optional(&mut **transaction)
        .await?;
    Ok(recipient_matches(recipient, current.as_deref()))
}

fn recipient_matches(recipient: &str, current: Option<&str>) -> bool {
    current == Some(recipient)
}

#[cfg(test)]
mod tests {
    use super::recipient_matches;

    #[test]
    fn login_key_reply_requires_the_current_exact_game_name() {
        assert!(recipient_matches("Player", Some("Player")));
        assert!(!recipient_matches("OldPlayer", Some("Player")));
        assert!(!recipient_matches("player", Some("Player")));
        assert!(!recipient_matches("Player", None));
    }
}
