use auction_application::AdminRuntimeDocument;

use super::forum::ForumSyncWorkerConfig;

use super::mooglemail::{
    MoogleMailEquipmentFilterConfig, MoogleMailWorkerConfig, MoogleMailWorldConfig,
};

/// 将 Admin 管理的旧版业务区块投影到 Worker 的 MoogleMail 启动快照。
/// transport endpoint、凭据来源和全局 `enabled` 仍由 Worker 主配置拥有；调度可由 operations/mail
/// 独立接管。此处的共享 runtime
/// 只决定旧版管理页实际可写的 world、发件人、装备过滤和通知模板。邮箱标识优先沿用主配置
/// 中同 world 的值，避免一次运营更新意外切换 durable cursor。
pub(in crate::config::settings) fn apply_runtime_document(
    config: &mut MoogleMailWorkerConfig,
    document: &AdminRuntimeDocument,
) {
    let (Some(worlds), Some(equipment_filter), Some(mail_templates)) = (
        document.worlds.as_ref(),
        document.equipment_filter.as_ref(),
        document.mail_templates.as_ref(),
    ) else {
        return;
    };

    let mut enabled_worlds = Vec::with_capacity(2);
    for (world_id, runtime) in [("main", &worlds.main), ("isekai", &worlds.isekai)] {
        if !runtime.enabled {
            continue;
        }
        // 只借用部署侧邮箱标识，不为查找复制所有发件人名单。
        let mailbox_key = if config.world_id == world_id {
            Some(config.mailbox_key.as_str())
        } else {
            config
                .additional_worlds
                .iter()
                .find(|world| world.world_id == world_id)
                .map(|world| world.mailbox_key.as_str())
        }
        .map(str::to_owned)
        .unwrap_or_else(|| format!("mooglemail:{world_id}"));
        enabled_worlds.push(MoogleMailWorldConfig {
            world_id: world_id.to_owned(),
            mailbox_key,
            sender_allowlist: runtime.sender_allowlist.clone(),
            sender_allowlist_disabled: runtime.sender_allowlist_disabled,
        });
    }

    let mut enabled_worlds = enabled_worlds.into_iter();
    // AdminRuntimeDocument::validate 已保证至少启用一个 world；这里仍失败关闭，避免未来调用者
    // 绕过 loader 后把 Worker 留在含义不明的空 world 状态。
    let Some(primary) = enabled_worlds.next() else {
        return;
    };
    config.world_id = primary.world_id;
    config.mailbox_key = primary.mailbox_key;
    config.sender_allowlist = primary.sender_allowlist;
    config.sender_allowlist_disabled = primary.sender_allowlist_disabled;
    config.additional_worlds = enabled_worlds.collect();
    config.equipment_filter = MoogleMailEquipmentFilterConfig {
        enabled: equipment_filter.enabled,
        qualities: if equipment_filter.qualities.is_empty() {
            auction_application::default_accepted_qualities()
        } else {
            equipment_filter.qualities.clone()
        },
        prefixes: equipment_filter.prefixes.clone(),
        suffixes: equipment_filter.suffixes.clone(),
        categories: equipment_filter.categories.clone(),
        equipment_types: equipment_filter.equipment_types.clone(),
        level_min: equipment_filter.level.min,
        level_max: equipment_filter.level.max,
    };
    config.mail_templates = mail_templates.clone();
}

/// 将 Admin runtime 的论坛装备标色规则投影到 Worker 论坛启动快照。
pub(in crate::config::settings) fn apply_equipment_colors(
    config: &mut ForumSyncWorkerConfig,
    document: &AdminRuntimeDocument,
) {
    // 保留单条规则中的多选维度；展开组合会改变规则优先级及运行时文档身份。
    config.equipment_colors =
        document.equipment_colors.as_ref().map(|colors| colors.to_rules()).unwrap_or_default();
}
