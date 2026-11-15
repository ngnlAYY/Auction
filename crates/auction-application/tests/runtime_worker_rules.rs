//! Admin 保存 runtime 文档时与 Worker 同口径的校验：装备过滤 token 与 Worker 加载器同一规则，
//! control socket 路径与 Worker 绑定同一规则；不合规的值在保存时就拒绝，不能留到 Worker
//! 下次启动或请求时才失败。

use auction_application::admin::{
    AdminRuntimeDocument, AdminRuntimeEquipmentFilterConfig, AdminRuntimeWorkerControlConfig,
    AdminRuntimeWorldsConfig,
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

#[test]
fn worker_control_socket_path_follows_the_binding_rule() {
    let document = |socket_path: &str| AdminRuntimeDocument {
        worker_control: AdminRuntimeWorkerControlConfig { socket_path: socket_path.to_owned() },
        ..AdminRuntimeDocument::default()
    };
    document("/run/hv-auction/worker/control.sock").validate().expect("默认路径必须被接受");
    let over_limit = format!("/run/{}", "s".repeat(96));
    for socket_path in ["/run/hv-auction/worker/", over_limit.as_str()] {
        assert!(document(socket_path).validate().is_err(), "{socket_path}");
    }
}
