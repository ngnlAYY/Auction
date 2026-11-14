//! Admin 保存 runtime 文档时的装备过滤 token 校验：与 Worker 加载器同一规则，
//! 含 Tab 等控制字符的 token 在保存时就拒绝，不能留到 Worker 下次启动才失败关闭。

use auction_application::admin::{
    AdminRuntimeDocument, AdminRuntimeEquipmentFilterConfig, AdminRuntimeWorldsConfig,
};

fn document_with_prefix(prefix: &str) -> AdminRuntimeDocument {
    AdminRuntimeDocument {
        worlds: Some(AdminRuntimeWorldsConfig::default()),
        equipment_filter: Some(AdminRuntimeEquipmentFilterConfig {
            enabled: true,
            prefixes: vec![prefix.to_owned()],
            ..AdminRuntimeEquipmentFilterConfig::default()
        }),
        ..AdminRuntimeDocument::default()
    }
}

#[test]
fn equipment_filter_tokens_with_control_characters_are_rejected() {
    // 从表格粘贴的一行会带内部 Tab；BEL 与 U+0085 同属控制字符。
    for token in ["Sav\tage", "Leg\u{7}endary", "A\u{85}B"] {
        assert!(document_with_prefix(token).validate().is_err(), "{token:?}");
    }
}

#[test]
fn canonical_equipment_filter_tokens_are_accepted() {
    for token in ["Savage", "Ethereal"] {
        document_with_prefix(token).validate().expect("规范 token 必须被接受");
    }
}
