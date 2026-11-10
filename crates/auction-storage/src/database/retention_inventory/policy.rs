//! 保留政策读取、校验与表/数据族聚合。政策 v2 引入运行态数据族的删除期限；
//! 期限只用于只读演练与后续经批准的清理，读取本身不提供任何变更能力。

use super::mapping::*;
use crate::database::retention_inventory::{
    PostgresRelationInventory, PostgresRetentionFamilyInventory, PostgresRetentionTableInventory,
    RETENTION_POLICY_JSON, RetentionInventoryError, RetentionPolicy, RetentionPolicyFamily,
};
use std::collections::BTreeMap;

/// 代码内闭集的清理规则；政策只能引用这些规则并给出期限，不能新增谓词。
pub(crate) struct RetentionRuleSpec {
    pub(crate) rule_id: &'static str,
    pub(crate) table: Option<&'static str>,
    pub(crate) basis: &'static str,
    pub(crate) duration_days: i64,
}

/// 与决策 23–25、29 一致的唯一规则集合。
pub(crate) const RETENTION_RULES: &[RetentionRuleSpec] = &[
    RetentionRuleSpec {
        rule_id: "service_heartbeat_expired_grace",
        table: Some("service_heartbeats"),
        basis: "expires_at",
        duration_days: 30,
    },
    RetentionRuleSpec {
        rule_id: "realtime_event_age",
        table: Some("realtime_events"),
        basis: "created_at",
        duration_days: 30,
    },
    RetentionRuleSpec {
        rule_id: "realtime_resolved_failure_age",
        table: Some("realtime_failures"),
        basis: "resolved_at",
        duration_days: 90,
    },
    RetentionRuleSpec {
        rule_id: "public_session_revoked",
        table: Some("public_sessions"),
        basis: "revoked_at",
        duration_days: 0,
    },
    RetentionRuleSpec {
        rule_id: "public_session_expired_grace",
        table: Some("public_sessions"),
        basis: "session_deadline",
        duration_days: 30,
    },
    RetentionRuleSpec {
        rule_id: "admin_session_revoked",
        table: Some("admin_sessions"),
        basis: "revoked_at",
        duration_days: 0,
    },
    RetentionRuleSpec {
        rule_id: "admin_session_expired_grace",
        table: Some("admin_sessions"),
        basis: "session_deadline",
        duration_days: 30,
    },
    RetentionRuleSpec {
        rule_id: "mail_diagnostic_ordinary_age",
        table: None,
        basis: "modified_at",
        duration_days: 7,
    },
    RetentionRuleSpec {
        rule_id: "mail_diagnostic_write_unknown_hold",
        table: None,
        basis: "disposition_plus_grace",
        duration_days: 90,
    },
];

pub(crate) fn retention_rule_spec(rule_id: &str) -> Option<&'static RetentionRuleSpec> {
    RETENTION_RULES.iter().find(|spec| spec.rule_id == rule_id)
}

pub(crate) fn load_retention_policy() -> Result<RetentionPolicy, RetentionInventoryError> {
    parse_retention_policy(RETENTION_POLICY_JSON)
}

/// 解析并校验政策；JSON 只在编译期内嵌，测试用同一入口验证闭集与期限绑定。
pub(crate) fn parse_retention_policy(
    source: &str,
) -> Result<RetentionPolicy, RetentionInventoryError> {
    let Ok(policy) = serde_json::from_str::<RetentionPolicy>(source) else {
        return Err(RetentionInventoryError::PolicyInvalid);
    };
    if policy.format != "auction-retention-policy.v2"
        || policy.policy_id != "auction-retention-policy"
        || policy.version != 2
        || policy.authority != "user-decisions-7-23-25-26-29-31"
        || policy.inventory_authorizes_mutation
        || !policy.deletion_requires_dry_run_and_operator_approval
        || !policy.audit_financial_and_business_facts_retain_forever
        || policy.families.is_empty()
        || policy.required_operator_decisions.is_empty()
    {
        return Err(RetentionInventoryError::PolicyInvalid);
    }
    validate_restore_objectives(&policy)?;
    validate_decisions(&policy)?;

    let mut seen_tables = BTreeMap::new();
    let mut seen_rules = BTreeMap::new();
    for family in &policy.families {
        if family.dataset_family.is_empty()
            || family.feature_owner.is_empty()
            || family.policy_state.is_empty()
            || family.candidate_class.is_empty()
            || family.mutation_authorized
        {
            return Err(RetentionInventoryError::PolicyInvalid);
        }
        for table in &family.tables {
            if !valid_identifier(table)
                || seen_tables.insert(table.clone(), family.dataset_family.clone()).is_some()
            {
                return Err(RetentionInventoryError::PolicyInvalid);
            }
        }
        validate_family_retention(family, &mut seen_rules)?;
        let filesystem =
            family.retention.as_ref().is_some_and(|retention| retention.kind == "filesystem");
        if family.tables.is_empty() && !filesystem {
            return Err(RetentionInventoryError::PolicyInvalid);
        }
    }
    if seen_rules.len() != RETENTION_RULES.len()
        || RETENTION_RULES.iter().any(|spec| !seen_rules.contains_key(spec.rule_id))
    {
        return Err(RetentionInventoryError::PolicyInvalid);
    }
    Ok(policy)
}

fn validate_restore_objectives(policy: &RetentionPolicy) -> Result<(), RetentionInventoryError> {
    let Some(objectives) = &policy.restore_objectives else {
        return Err(RetentionInventoryError::PolicyInvalid);
    };
    if !(1..=3_600).contains(&objectives.rpo_seconds)
        || !(1..=86_400).contains(&objectives.rto_seconds)
        || !objectives.wal_archive_enabled
        || !(1..=35).contains(&objectives.point_in_time_recovery_window_days)
        || !valid_identifier(&objectives.rehearsal_cadence)
    {
        return Err(RetentionInventoryError::PolicyInvalid);
    }
    Ok(())
}

fn validate_decisions(policy: &RetentionPolicy) -> Result<(), RetentionInventoryError> {
    if policy
        .required_operator_decisions
        .iter()
        .chain(policy.resolved_operator_decisions.iter())
        .any(|decision| !valid_identifier(decision))
        || policy.required_operator_decisions.iter().any(|decision| {
            policy.resolved_operator_decisions.iter().any(|resolved| resolved == decision)
        })
    {
        return Err(RetentionInventoryError::PolicyInvalid);
    }
    Ok(())
}

fn validate_family_retention(
    family: &RetentionPolicyFamily,
    seen_rules: &mut BTreeMap<String, String>,
) -> Result<(), RetentionInventoryError> {
    let Some(retention) = &family.retention else {
        if family.policy_state == "retain_forever" {
            return Ok(());
        }
        return Err(RetentionInventoryError::PolicyInvalid);
    };
    if family.policy_state == "retain_forever"
        || retention.rules.is_empty()
        || !matches!(retention.kind.as_str(), "database_rows" | "filesystem")
    {
        return Err(RetentionInventoryError::PolicyInvalid);
    }
    for table in &retention.retain_forever {
        if !valid_identifier(table) || !family.tables.contains(table) {
            return Err(RetentionInventoryError::PolicyInvalid);
        }
    }
    for rule in &retention.rules {
        let Some(spec) = retention_rule_spec(&rule.rule_id) else {
            return Err(RetentionInventoryError::PolicyInvalid);
        };
        if rule.basis != spec.basis
            || rule.duration_days != spec.duration_days
            || rule.table.as_deref() != spec.table
            || seen_rules.insert(rule.rule_id.clone(), family.dataset_family.clone()).is_some()
        {
            return Err(RetentionInventoryError::PolicyInvalid);
        }
        match (retention.kind.as_str(), rule.table.as_deref()) {
            ("database_rows", Some(table)) if family.tables.contains(&table.to_owned()) => {}
            ("filesystem", None) => {}
            _ => return Err(RetentionInventoryError::PolicyInvalid),
        }
    }
    Ok(())
}

pub(crate) fn build_policy_table_inventory(
    policy: &RetentionPolicy,
    relations: &[PostgresRelationInventory],
) -> Vec<PostgresRetentionTableInventory> {
    let relation_by_name = relations
        .iter()
        .filter(|relation| relation.schema_name == "public")
        .map(|relation| (relation.relation_name.as_str(), relation))
        .collect::<BTreeMap<_, _>>();
    let mut tables = Vec::new();
    for family in &policy.families {
        for table_name in &family.tables {
            let relation = relation_by_name.get(table_name.as_str()).copied();
            tables.push(PostgresRetentionTableInventory {
                dataset_family: family.dataset_family.clone(),
                feature_owner: family.feature_owner.clone(),
                policy_state: family.policy_state.clone(),
                candidate_class: family.candidate_class.clone(),
                table_name: table_name.clone(),
                relation_present: relation.is_some(),
                estimated_rows: relation.map(|value| value.estimated_rows),
                estimated_dead_rows: relation.map(|value| value.estimated_dead_rows),
                catalog_rows_estimate: relation.map(|value| value.catalog_rows_estimate),
                last_analyze: relation.and_then(|value| value.last_analyze),
                last_autoanalyze: relation.and_then(|value| value.last_autoanalyze),
                table_bytes: relation.map(|value| value.table_bytes),
                index_bytes: relation.map(|value| value.index_bytes),
                total_bytes: relation.map(|value| value.total_bytes),
                eligibility_assessed: false,
                eligibility_authorized: false,
            });
        }
    }
    tables
}

pub(crate) fn build_policy_family_inventory(
    policy: &RetentionPolicy,
    tables: &[PostgresRetentionTableInventory],
) -> Result<Vec<PostgresRetentionFamilyInventory>, RetentionInventoryError> {
    policy
        .families
        .iter()
        .map(|family| -> Result<PostgresRetentionFamilyInventory, RetentionInventoryError> {
            let family_tables =
                tables.iter().filter(|table| table.dataset_family == family.dataset_family);
            let mut configured_table_count = 0_i64;
            let mut present_table_count = 0_i64;
            let mut estimated_rows = 0_i64;
            let mut estimated_dead_rows = 0_i64;
            let mut total_bytes = 0_i64;
            let mut statistics_complete = true;
            for table in family_tables {
                configured_table_count = configured_table_count
                    .checked_add(1)
                    .ok_or(RetentionInventoryError::CountOverflow)?;
                if table.relation_present {
                    present_table_count = present_table_count
                        .checked_add(1)
                        .ok_or(RetentionInventoryError::CountOverflow)?;
                } else {
                    statistics_complete = false;
                }
                let Some(rows) = table.estimated_rows else {
                    statistics_complete = false;
                    continue;
                };
                estimated_rows = estimated_rows
                    .checked_add(rows.max(0))
                    .ok_or(RetentionInventoryError::CountOverflow)?;
                let Some(dead_rows) = table.estimated_dead_rows else {
                    statistics_complete = false;
                    continue;
                };
                estimated_dead_rows = estimated_dead_rows
                    .checked_add(dead_rows.max(0))
                    .ok_or(RetentionInventoryError::CountOverflow)?;
                let Some(bytes) = table.total_bytes else {
                    statistics_complete = false;
                    continue;
                };
                total_bytes = total_bytes
                    .checked_add(bytes.max(0))
                    .ok_or(RetentionInventoryError::CountOverflow)?;
            }
            Ok(PostgresRetentionFamilyInventory {
                dataset_family: family.dataset_family.clone(),
                feature_owner: family.feature_owner.clone(),
                policy_state: family.policy_state.clone(),
                candidate_class: family.candidate_class.clone(),
                configured_table_count,
                present_table_count,
                estimated_rows,
                estimated_dead_rows,
                total_bytes,
                statistics_complete,
                eligibility_assessed: false,
                eligibility_authorized: false,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{RETENTION_POLICY_JSON, RetentionInventoryError, parse_retention_policy};

    fn policy_value() -> serde_json::Value {
        serde_json::from_str(RETENTION_POLICY_JSON).expect("内嵌政策是合法 JSON")
    }

    fn set_rule_duration(value: &mut serde_json::Value, rule_id: &str, days: i64) {
        let families = value["families"].as_array_mut().expect("families");
        for family in families {
            let Some(rules) = family
                .get_mut("retention")
                .and_then(|retention| retention.get_mut("rules"))
                .and_then(|rules| rules.as_array_mut())
            else {
                continue;
            };
            for rule in rules {
                if rule["rule_id"] == rule_id {
                    rule["duration_days"] = serde_json::json!(days);
                    return;
                }
            }
        }
        panic!("未找到规则 {rule_id}");
    }

    #[test]
    fn accepts_the_embedded_v2_policy() {
        let policy = parse_retention_policy(RETENTION_POLICY_JSON).expect("政策 v2");
        assert_eq!(policy.version, 2);
        assert!(policy.deletion_requires_dry_run_and_operator_approval);
        assert!(policy.restore_objectives.is_some());
    }

    #[test]
    fn omits_schema_objects_removed_by_migration_097() {
        let policy = parse_retention_policy(RETENTION_POLICY_JSON).expect("政策 v2");
        for dropped in [
            "bid_proxies",
            "bid_proxy_attempts",
            "bid_proxy_outbid_uses",
            "auction_forum_reply_cursors",
        ] {
            assert!(
                policy
                    .families
                    .iter()
                    .all(|family| family.tables.iter().all(|table| table != dropped)),
                "保留策略不得再登记已删除对象 {dropped}"
            );
        }
        // 政策版本必须跟随迁移链；写死数字会在追加迁移后静默过期。
        assert_eq!(
            policy.database_schema_version,
            crate::database::postgres_expected_migration_version()
        );
        assert!(
            policy
                .resolved_operator_decisions
                .iter()
                .any(|decision| decision == "unreferenced_schema_object_deletion")
        );
        assert!(
            policy.families.iter().any(|family| family
                .tables
                .iter()
                .any(|table| table == "operations_alert_webhook_deliveries")),
            "保留策略必须登记 W5.1 webhook 运行态表"
        );
    }

    #[test]
    fn rejects_a_duration_that_diverges_from_the_decisions() {
        let mut value = policy_value();
        set_rule_duration(&mut value, "service_heartbeat_expired_grace", 31);
        assert!(matches!(
            parse_retention_policy(&value.to_string()),
            Err(RetentionInventoryError::PolicyInvalid)
        ));
    }

    #[test]
    fn rejects_unknown_cleanup_rules() {
        let mut value = policy_value();
        let heartbeat = value["families"]
            .as_array_mut()
            .expect("families")
            .iter_mut()
            .find(|family| family["dataset_family"] == "service_heartbeat")
            .expect("heartbeat family");
        heartbeat["retention"]["rules"].as_array_mut().expect("rules").push(serde_json::json!({
            "rule_id": "service_heartbeat_forever",
            "table": "service_heartbeats",
            "basis": "expires_at",
            "duration_days": 0
        }));
        assert!(matches!(
            parse_retention_policy(&value.to_string()),
            Err(RetentionInventoryError::PolicyInvalid)
        ));
    }

    #[test]
    fn rejects_retention_rules_on_a_retain_forever_family() {
        let mut value = policy_value();
        let families = value["families"].as_array_mut().expect("families");
        let audit_retention = families
            .iter()
            .find(|family| family["dataset_family"] == "service_heartbeat")
            .and_then(|family| family.get("retention"))
            .cloned()
            .expect("heartbeat retention");
        families
            .iter_mut()
            .find(|family| family["dataset_family"] == "audit")
            .expect("audit family")["retention"] = audit_retention;
        assert!(matches!(
            parse_retention_policy(&value.to_string()),
            Err(RetentionInventoryError::PolicyInvalid)
        ));
    }

    #[test]
    fn rejects_a_missing_dry_run_requirement() {
        let mut value = policy_value();
        value["deletion_requires_dry_run_and_operator_approval"] = serde_json::json!(false);
        assert!(matches!(
            parse_retention_policy(&value.to_string()),
            Err(RetentionInventoryError::PolicyInvalid)
        ));
    }
}
