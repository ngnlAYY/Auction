use auction_contracts::AdminCredentialResponse;

/// 将应用层凭据转换为不包含 secret 的管理端安全 DTO。
pub(crate) fn map_credential(
    credential: auction_application::admin::AdminCredential,
) -> AdminCredentialResponse {
    AdminCredentialResponse {
        id: credential.id.to_string(),
        kind: credential.kind.as_str().to_owned(),
        world: credential.world.as_str().to_owned(),
        label: credential.label,
        player_name: credential.player_name,
        status: credential.status.as_str().to_owned(),
        state_version: credential.state_version,
        created_at: super::timestamp(credential.created_at),
        rotated_at: credential.rotated_at.map(super::timestamp),
        updated_at: super::timestamp(credential.updated_at),
    }
}
