use auction_application::DeliveryAdminSnapshot;
use auction_contracts::AdminDeliveryResponse;

/// 将 delivery 应用快照转换为管理端安全 DTO。
pub(crate) fn map_snapshot(snapshot: DeliveryAdminSnapshot) -> AdminDeliveryResponse {
    AdminDeliveryResponse {
        id: snapshot.id.to_string(),
        origin_kind: snapshot.origin_kind,
        origin_id: snapshot.origin_id,
        recipient: snapshot.recipient,
        cod: snapshot.cod,
        status: snapshot.status,
        lease_expires_at: snapshot.lease_expires_at.map(super::timestamp),
        fencing_version: snapshot.fencing_version,
        attempt_count: snapshot.attempt_count,
        claimed_at: snapshot.claimed_at.map(super::timestamp),
        sending_at: snapshot.sending_at.map(super::timestamp),
        completed_at: snapshot.completed_at.map(super::timestamp),
        external_reference: snapshot.external_reference,
        failure_code: snapshot.failure_code,
        reason_code: snapshot.reason_code,
        take_required: snapshot.take_required,
        returned_mail_id: snapshot.returned_mail_id,
        created_at: super::timestamp(snapshot.created_at),
        updated_at: super::timestamp(snapshot.updated_at),
    }
}
