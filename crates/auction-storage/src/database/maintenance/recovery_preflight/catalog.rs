//! 首次初始化和恢复共用的 PostgreSQL 用户对象盘点；不读取敏感 option。

/// 只允许默认系统对象与空 public schema；共享订阅目录必须限定当前数据库。
pub(crate) const USER_OBJECT_COUNT_SQL: &str = "WITH user_namespaces AS (
                 SELECT oid, nspname
                 FROM pg_catalog.pg_namespace
                 WHERE nspname NOT IN ('pg_catalog', 'information_schema')
                   AND nspname !~ '^pg_(toast|temp)'
             ),
             user_objects AS (
                 SELECT oid FROM user_namespaces WHERE nspname <> 'public'
                 UNION ALL
                 SELECT class_record.oid
                 FROM pg_catalog.pg_class AS class_record
                 JOIN user_namespaces AS namespace_record
                   ON namespace_record.oid = class_record.relnamespace
                 UNION ALL
                 SELECT type_record.oid
                 FROM pg_catalog.pg_type AS type_record
                 JOIN user_namespaces AS namespace_record
                   ON namespace_record.oid = type_record.typnamespace
                 UNION ALL
                 SELECT procedure_record.oid
                 FROM pg_catalog.pg_proc AS procedure_record
                 JOIN user_namespaces AS namespace_record
                   ON namespace_record.oid = procedure_record.pronamespace
                 UNION ALL
                 SELECT collation_record.oid
                 FROM pg_catalog.pg_collation AS collation_record
                 JOIN user_namespaces AS namespace_record
                   ON namespace_record.oid = collation_record.collnamespace
                 UNION ALL
                 SELECT conversion_record.oid
                 FROM pg_catalog.pg_conversion AS conversion_record
                 JOIN user_namespaces AS namespace_record
                   ON namespace_record.oid = conversion_record.connamespace
                 UNION ALL
                 SELECT operator_record.oid
                 FROM pg_catalog.pg_operator AS operator_record
                 JOIN user_namespaces AS namespace_record
                   ON namespace_record.oid = operator_record.oprnamespace
                 UNION ALL
                 SELECT operator_class_record.oid
                 FROM pg_catalog.pg_opclass AS operator_class_record
                 JOIN user_namespaces AS namespace_record
                   ON namespace_record.oid = operator_class_record.opcnamespace
                 UNION ALL
                 SELECT operator_family_record.oid
                 FROM pg_catalog.pg_opfamily AS operator_family_record
                 JOIN user_namespaces AS namespace_record
                   ON namespace_record.oid = operator_family_record.opfnamespace
                 UNION ALL
                 SELECT configuration_record.oid
                 FROM pg_catalog.pg_ts_config AS configuration_record
                 JOIN user_namespaces AS namespace_record
                   ON namespace_record.oid = configuration_record.cfgnamespace
                 UNION ALL
                 SELECT dictionary_record.oid
                 FROM pg_catalog.pg_ts_dict AS dictionary_record
                 JOIN user_namespaces AS namespace_record
                   ON namespace_record.oid = dictionary_record.dictnamespace
                 UNION ALL
                 SELECT parser_record.oid
                 FROM pg_catalog.pg_ts_parser AS parser_record
                 JOIN user_namespaces AS namespace_record
                   ON namespace_record.oid = parser_record.prsnamespace
                 UNION ALL
                 SELECT template_record.oid
                 FROM pg_catalog.pg_ts_template AS template_record
                 JOIN user_namespaces AS namespace_record
                   ON namespace_record.oid = template_record.tmplnamespace
                 UNION ALL
                 SELECT extension_record.oid
                 FROM pg_catalog.pg_extension AS extension_record
                 WHERE extension_record.extname <> 'plpgsql'
                 UNION ALL
                 SELECT wrapper_record.oid
                 FROM pg_catalog.pg_foreign_data_wrapper AS wrapper_record
                 UNION ALL
                 SELECT server_record.oid
                 FROM pg_catalog.pg_foreign_server AS server_record
                 UNION ALL
                 SELECT mapping_record.umid
                 FROM pg_catalog.pg_user_mappings AS mapping_record
                 UNION ALL
                 SELECT trigger_record.oid
                 FROM pg_catalog.pg_event_trigger AS trigger_record
                 UNION ALL
                 SELECT publication_record.oid
                 FROM pg_catalog.pg_publication AS publication_record
                 UNION ALL
                 SELECT subscription_record.oid
                 FROM pg_catalog.pg_subscription AS subscription_record
                 WHERE subscription_record.subdbid = (
                     SELECT oid FROM pg_catalog.pg_database WHERE datname = current_database()
                 )
                 UNION ALL
                 SELECT object_record.oid
                 FROM pg_catalog.pg_largeobject_metadata AS object_record
                 UNION ALL
                 SELECT acl_record.oid
                 FROM pg_catalog.pg_default_acl AS acl_record
             )
SELECT COUNT(*)::bigint FROM user_objects";
