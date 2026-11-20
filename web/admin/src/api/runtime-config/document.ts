// 运行配置文档的业务段校验：世界、装备颜色与筛选、模板、装备来源与看板邮箱。

import { type JsonObject, jsonExactObject } from "@auction/web-shared/net/wire";

import {
  boolField,
  fieldValue,
  optionalKeysObject,
  safeI64,
  stringField,
  validScalars,
  validStringArray,
} from "./fields";
import {
  validListener,
  validMetrics,
  validSecurity,
  validSession,
  validTracing,
  validWorkerControl,
} from "./infrastructure";
import { validOperations } from "./operations";

/** 校验完整运行配置文档的顶层闭集与各业务区块；兼容字段可以缺省。 */
export function validRuntimeDocument(value: unknown): boolean {
  const document = optionalKeysObject(value, [
    "allow_manual_early_open",
    "allow_manual_early_settlement",
    "bot_players",
    "dashboard_mailboxes",
    "equipment_colors",
    "equipment_filter",
    "equipment_source",
    "listener",
    "mail_templates",
    "metrics",
    "operations",
    "security",
    "session",
    "tracing",
    "worker_control",
    "worlds",
  ]);
  if (document === null) {
    return false;
  }
  return (
    boolField(document, "allow_manual_early_open") !== null &&
    boolField(document, "allow_manual_early_settlement") !== null &&
    optionalField(document, "bot_players", validStringArray) &&
    optionalField(document, "equipment_colors", validEquipmentColors) &&
    requiredField(document, "worlds", validWorlds) &&
    requiredField(document, "equipment_filter", validEquipmentFilter) &&
    optionalField(document, "mail_templates", validLegacyMailTemplates) &&
    requiredField(document, "equipment_source", validEquipmentSource) &&
    requiredField(document, "listener", validListener) &&
    requiredField(document, "dashboard_mailboxes", validDashboardMailboxes) &&
    requiredField(document, "metrics", validMetrics) &&
    requiredField(document, "security", validSecurity) &&
    requiredField(document, "session", validSession) &&
    requiredField(document, "tracing", validTracing) &&
    requiredField(document, "worker_control", validWorkerControl) &&
    requiredField(document, "operations", validOperations)
  );
}

function validWorlds(value: unknown): boolean {
  const worlds = jsonExactObject(value, ["isekai", "main"]);
  return worlds !== null && Object.values(worlds).every(validRuntimeWorld);
}

function validRuntimeWorld(value: unknown): boolean {
  return (
    validScalars(value, ["enabled", "sender_allowlist_disabled"], [], [], ["sender_allowlist"]) &&
    validStringArray(fieldValue(value, "sender_allowlist"))
  );
}

function validEquipmentColors(value: unknown): boolean {
  const colors = jsonExactObject(value, ["rules"]);
  if (colors === null) {
    return false;
  }
  const rules = colors.rules;
  if (!Array.isArray(rules)) {
    return false;
  }
  // 每条规则展开完整装备词典的复选组，限制编辑器一次构建的控件数量。
  return rules.length <= 200 && rules.every(validEquipmentColorRule);
}

function validEquipmentColorRule(value: unknown): boolean {
  const rule = optionalKeysObject(value, [
    "categories",
    "category",
    "combined_color",
    "equipment_type",
    "equipment_types",
    "level",
    "prefix_color",
    "prefixes",
    "quality",
    "qualities",
    "suffix_color",
    "suffixes",
  ]);
  if (rule === null) {
    return false;
  }
  return (
    [
      "category",
      "equipment_type",
      "quality",
      "prefix_color",
      "suffix_color",
      "combined_color",
    ].every((key) => stringField(rule, key) !== null) &&
    ["categories", "equipment_types", "qualities"].every((key) =>
      optionalField(rule, key, validStringArray),
    ) &&
    requiredField(rule, "level", validLevel) &&
    ["prefixes", "suffixes"].every((key) => requiredField(rule, key, validStringArray))
  );
}

function validLevel(value: unknown): boolean {
  const level = jsonExactObject(value, ["max", "min"]);
  return level !== null && Object.values(level).every((item) => safeI64(item) !== null);
}

function validEquipmentFilter(value: unknown): boolean {
  const lists = ["qualities", "prefixes", "suffixes", "categories", "equipment_types"];
  return (
    validScalars(
      value,
      ["enabled"],
      [],
      [],
      ["categories", "equipment_types", "level", "prefixes", "qualities", "suffixes"],
    ) &&
    validLevel(fieldValue(value, "level")) &&
    lists.every((key) => validStringArray(fieldValue(value, key)))
  );
}

/** 已迁入数据库的历史模板原样往返，浏览器只核对对象形状。 */
function validLegacyMailTemplates(value: unknown): boolean {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return false;
  }
  return Object.values(value).every((item) => typeof item === "string");
}

function validEquipmentSource(value: unknown): boolean {
  return (
    validScalars(
      value,
      ["allowlist_enabled", "require_tradeable"],
      [],
      ["maximum_response_bytes", "minimum_request_interval_millis", "request_timeout_seconds"],
      ["allowed_hosts", "allowed_path_prefixes"],
    ) &&
    validStringArray(fieldValue(value, "allowed_hosts")) &&
    validStringArray(fieldValue(value, "allowed_path_prefixes"))
  );
}

function validDashboardMailboxes(value: unknown): boolean {
  return (
    Array.isArray(value) &&
    value.every(
      (mailbox) =>
        validScalars(mailbox, [], ["mailbox_key", "world_id"], [], []) &&
        isWorldId(fieldValue(mailbox, "world_id")),
    )
  );
}

function isWorldId(value: unknown): boolean {
  return value === "main" || value === "isekai";
}

/** Rust `Option::is_none_or`：缺键合法，出现时由谓词决定。 */
function optionalField(object: JsonObject, key: string, predicate: (value: unknown) => boolean) {
  return !Object.hasOwn(object, key) || predicate(object[key]);
}

/** Rust `Option::is_some_and`：键必须出现且通过谓词。 */
function requiredField(object: JsonObject, key: string, predicate: (value: unknown) => boolean) {
  return Object.hasOwn(object, key) && predicate(object[key]);
}
