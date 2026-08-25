//! 保留政策读取、校验与表/数据族聚合。

use super::mapping::*;
use super::*;
use std::collections::BTreeMap;

pub(super) fn load_retention_policy() -> Result<RetentionPolicy, RetentionInventoryError> {
    let policy: RetentionPolicy = serde_json::from_str(RETENTION_POLICY_JSON)
        .map_err(|_| RetentionInventoryError::PolicyInvalid)?;
    if policy.format != "auction-retention-policy.v1"
        || policy.policy_id != "auction-retention-policy"
        || policy.version != 1
        || policy.authority != "repository-conservative-default"
        || policy.inventory_authorizes_mutation
        || policy.families.is_empty()
        || policy.required_operator_decisions.is_empty()
    {
        return Err(RetentionInventoryError::PolicyInvalid);
    }

    let mut seen_tables = BTreeMap::new();
    for family in &policy.families {
        if family.dataset_family.is_empty()
            || family.feature_owner.is_empty()
            || family.policy_state.is_empty()
            || family.candidate_class.is_empty()
            || family.mutation_authorized
            || family.tables.is_empty()
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
    }
    if policy.required_operator_decisions.iter().any(|decision| !valid_identifier(decision)) {
        return Err(RetentionInventoryError::PolicyInvalid);
    }
    Ok(policy)
}

pub(super) fn build_policy_table_inventory(
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

pub(super) fn build_policy_family_inventory(
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
