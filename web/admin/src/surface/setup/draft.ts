// 首次部署的页面内草稿与字段元数据；保留原始数字输入，不持久化或格式化凭据。

import { DEFAULT_SETUP_SCHEDULE, type SetupDatabaseRequest } from "@auction/contracts/setup";
import { createUuidV4 } from "@auction/web-shared/platform/uuid";
import { unreachable } from "@auction/web-shared/result";

/** 字段顺序同时决定渲染、错误摘要和首个错误焦点的顺序。 */
export type SetupField =
  | "database_host"
  | "database_port"
  | "database_username"
  | "database_password"
  | "database_name"
  | "admin_actor"
  | "admin_secret"
  | "admin_confirmation"
  | "public_origin"
  | "admin_origin"
  | "public_listener_port"
  | "admin_listener_port"
  | "schedule_timezone"
  | "schedule_preview_day"
  | "schedule_preview_time"
  | "schedule_open_day"
  | "schedule_open_time"
  | "schedule_end_day"
  | "schedule_end_time"
  | "start_price"
  | "log_filter"
  | "worker_concurrency"
  | "worker_poll"
  | "realtime_poll";

/** 合并部署确认快照包含的目标字段。 */
export const SETUP_FIELD_DATABASE_TARGET: readonly SetupField[] = [
  "database_host",
  "database_port",
  "database_name",
];

/** 数据库连接输入的渲染与校验顺序。 */
export const SETUP_FIELD_DATABASE: readonly SetupField[] = [
  "database_host",
  "database_port",
  "database_username",
  "database_password",
  "database_name",
];

/** 管理员账号与两处密钥确认字段。 */
export const SETUP_FIELD_ADMIN: readonly SetupField[] = [
  "admin_actor",
  "admin_secret",
  "admin_confirmation",
];

/** 站点 Origin 与起拍价等基础字段。 */
export const SETUP_FIELD_BASIC: readonly SetupField[] = [
  "public_origin",
  "admin_origin",
  "start_price",
];

/** Public 与 Admin 的回环监听端口。 */
export const SETUP_FIELD_LISTENER: readonly SetupField[] = [
  "public_listener_port",
  "admin_listener_port",
];

/** 本地周排期的时区、预览与开拍/结束时间字段。 */
export const SETUP_FIELD_SCHEDULE: readonly SetupField[] = [
  "schedule_timezone",
  "schedule_preview_day",
  "schedule_preview_time",
  "schedule_open_day",
  "schedule_open_time",
  "schedule_end_day",
  "schedule_end_time",
];

/** 星期选项的线上值与对应译文键；顺序即渲染顺序。 */
export const SETUP_FIELD_WEEKDAYS: readonly (readonly [string, string])[] = [
  ["Monday", "setup-day-monday"],
  ["Tuesday", "setup-day-tuesday"],
  ["Wednesday", "setup-day-wednesday"],
  ["Thursday", "setup-day-thursday"],
  ["Friday", "setup-day-friday"],
  ["Saturday", "setup-day-saturday"],
  ["Sunday", "setup-day-sunday"],
];

/** 默认折叠的性能与日志字段。 */
export const SETUP_FIELD_ADVANCED: readonly SetupField[] = [
  "log_filter",
  "worker_concurrency",
  "worker_poll",
  "realtime_poll",
];

/** 全部字段；顺序就是渲染、错误摘要和首个错误焦点的顺序。 */
export const SETUP_FIELDS: readonly SetupField[] = [
  ...SETUP_FIELD_DATABASE,
  ...SETUP_FIELD_ADMIN,
  ...SETUP_FIELD_BASIC,
  ...SETUP_FIELD_LISTENER,
  ...SETUP_FIELD_SCHEDULE,
  ...SETUP_FIELD_ADVANCED,
];

/** 控件 ID 复用既有标签键，辅助文案固定使用 `-help` 后缀。 */
export function setupFieldId(field: SetupField): string {
  switch (field) {
    case "database_host":
      return "setup-database-host";
    case "database_port":
      return "setup-database-port";
    case "database_username":
      return "setup-database-username";
    case "database_password":
      return "setup-database-password";
    case "database_name":
      return "setup-database-name";
    case "admin_actor":
      return "setup-admin-actor";
    case "admin_secret":
      return "setup-admin-secret";
    case "admin_confirmation":
      return "setup-admin-confirmation";
    case "public_origin":
      return "setup-public-origin";
    case "admin_origin":
      return "setup-admin-origin";
    case "public_listener_port":
      return "setup-public-listener-port";
    case "admin_listener_port":
      return "setup-admin-listener-port";
    case "schedule_timezone":
      return "setup-schedule-timezone";
    case "schedule_preview_day":
      return "setup-schedule-preview-day";
    case "schedule_preview_time":
      return "setup-schedule-preview-time";
    case "schedule_open_day":
      return "setup-schedule-open-day";
    case "schedule_open_time":
      return "setup-schedule-open-time";
    case "schedule_end_day":
      return "setup-schedule-end-day";
    case "schedule_end_time":
      return "setup-schedule-end-time";
    case "start_price":
      return "setup-start-price";
    case "log_filter":
      return "setup-log-filter";
    case "worker_concurrency":
      return "setup-worker-concurrency";
    case "worker_poll":
      return "setup-worker-poll";
    case "realtime_poll":
      return "setup-realtime-poll";
    default:
      return unreachable(field);
  }
}

/** 数字输入的上下界与步长；不是数字字段时返回 `null`。 */
export function setupFieldNumberBounds(
  field: SetupField,
): readonly [minimum: number, maximum: number, step: number] | null {
  switch (field) {
    case "database_port":
    case "public_listener_port":
    case "admin_listener_port":
      return [1, 65_535, 1];
    case "start_price":
      return [1_000, 1_000_000_000, 1_000];
    case "worker_concurrency":
      return [1, 8, 1];
    case "worker_poll":
      return [11, 300, 1];
    case "realtime_poll":
      return [100, 5_000, 1];
    default:
      return null;
  }
}

/** 与配置 owner 的 UTF-8 字节上限对齐；HTML maxlength 仅作宽松输入辅助。 */
export function setupFieldMaximumBytes(field: SetupField): number | null {
  switch (field) {
    case "database_host":
      return 253;
    case "database_username":
    case "database_name":
    case "admin_actor":
      return 128;
    case "database_password":
      return 4_096;
    case "admin_secret":
    case "admin_confirmation":
      return 512;
    case "log_filter":
      return 256;
    case "schedule_timezone":
      return 128;
    default:
      if (setupFieldIsWeekday(field)) {
        return 9;
      }
      if (setupFieldIsScheduleTime(field)) {
        return 5;
      }
      return null;
  }
}

/** 控件的 HTML `type`；密码字段在揭示时由调用方改为文本。 */
export function setupFieldInputType(
  field: SetupField,
): "password" | "url" | "time" | "number" | "text" {
  switch (field) {
    case "database_password":
    case "admin_secret":
    case "admin_confirmation":
      return "password";
    case "public_origin":
    case "admin_origin":
      return "url";
    default:
      if (setupFieldIsScheduleTime(field)) {
        return "time";
      }
      if (setupFieldNumberBounds(field) !== null) {
        return "number";
      }
      return "text";
  }
}

/** 浏览器自动填充提示；数据库密码与管理员密钥都按新密码处理。 */
export function setupFieldAutocomplete(field: SetupField): "username" | "new-password" | "off" {
  switch (field) {
    case "admin_actor":
      return "username";
    case "database_password":
    case "admin_secret":
    case "admin_confirmation":
      return "new-password";
    default:
      return "off";
  }
}

/** 星期字段使用固定下拉选项，不交给自由文本输入。 */
export function setupFieldIsWeekday(field: SetupField): boolean {
  return (
    field === "schedule_preview_day" ||
    field === "schedule_open_day" ||
    field === "schedule_end_day"
  );
}

/** 排期时间字段使用时间输入与 60 秒步长。 */
export function setupFieldIsScheduleTime(field: SetupField): boolean {
  return (
    field === "schedule_preview_time" ||
    field === "schedule_open_time" ||
    field === "schedule_end_time"
  );
}

/** 联动字段修改时一并清除旧错误，避免密码、Origin、端口或排期顺序提示过期。 */
export function setupFieldRelated(field: SetupField, other: SetupField): boolean {
  return (
    field === other ||
    ((field === "admin_secret" || field === "admin_confirmation") &&
      (other === "admin_secret" || other === "admin_confirmation")) ||
    ((field === "public_origin" || field === "admin_origin") &&
      (other === "public_origin" || other === "admin_origin")) ||
    (SETUP_FIELD_LISTENER.includes(field) && SETUP_FIELD_LISTENER.includes(other)) ||
    (SETUP_FIELD_SCHEDULE.includes(field) && SETUP_FIELD_SCHEDULE.includes(other))
  );
}

/** 合并部署初始化确认只包含数据库地址和库名。 */
export interface DatabaseTarget {
  readonly host: string;
  readonly port: number;
  readonly name: string;
}

/** 确认快照与待提交的数据库输入是否指向同一目标。 */
export function databaseTargetMatches(
  target: DatabaseTarget,
  database: SetupDatabaseRequest,
): boolean {
  return (
    target.host === database.host && target.port === database.port && target.name === database.name
  );
}

/** 比较两个确认快照：两个 `null` 都表示未确认，视为相等；恰有一方为 `null` 或三元组不同则为 `false`。 */
export function databaseTargetsEqual(
  left: DatabaseTarget | null,
  right: DatabaseTarget | null,
): boolean {
  return (
    left === right ||
    (left !== null &&
      right !== null &&
      left.host === right.host &&
      left.port === right.port &&
      left.name === right.name)
  );
}

/** 固定字段集的草稿；整份草稿和字段值含密码，只在当前设置会话内存中使用，不得记录、持久化或输出调试信息。 */
export interface SetupDraft {
  readonly values: Readonly<Record<SetupField, string>>;
  readonly metricsEnabled: boolean;
  readonly scheduleEnabled: boolean;
  readonly confirmedDatabaseTarget: DatabaseTarget | null;
}

const EMPTY_SETUP_VALUES: Readonly<Record<SetupField, string>> = {
  database_host: "",
  database_port: "",
  database_username: "",
  database_password: "",
  database_name: "",
  admin_actor: "",
  admin_secret: "",
  admin_confirmation: "",
  public_origin: "",
  admin_origin: "",
  public_listener_port: "",
  admin_listener_port: "",
  schedule_timezone: "",
  schedule_preview_day: "",
  schedule_preview_time: "",
  schedule_open_day: "",
  schedule_open_time: "",
  schedule_end_day: "",
  schedule_end_time: "",
  start_price: "",
  log_filter: "",
  worker_concurrency: "",
  worker_poll: "",
  realtime_poll: "",
};

const DEFAULT_SETUP_VALUES: readonly (readonly [SetupField, string])[] = [
  ["database_host", "127.0.0.1"],
  ["database_port", "5432"],
  ["database_name", "auction"],
  ["public_listener_port", "8090"],
  ["admin_listener_port", "8091"],
  ["start_price", "1000"],
  ["log_filter", "info,auction=debug"],
  ["worker_concurrency", "4"],
  ["worker_poll", "15"],
  ["realtime_poll", "250"],
];

/** 创建本次设置会话的默认草稿，并为管理员账号生成一对随机密钥。 */
export function createSetupDraft(): SetupDraft {
  let draft: SetupDraft = {
    values: EMPTY_SETUP_VALUES,
    metricsEnabled: true,
    scheduleEnabled: false,
    confirmedDatabaseTarget: null,
  };
  for (const [field, value] of DEFAULT_SETUP_VALUES) {
    draft = setupDraftSet(draft, field, value);
  }
  draft = setupDraftGenerateAdminSecret(draft);
  // 排期草稿与首次 wire 的兼容默认值一致，恢复进阶参数不触碰这些值。
  for (const [field, value] of [
    ["schedule_timezone", DEFAULT_SETUP_SCHEDULE.timezone],
    ["schedule_preview_day", DEFAULT_SETUP_SCHEDULE.preview_day],
    ["schedule_preview_time", DEFAULT_SETUP_SCHEDULE.preview_time],
    ["schedule_open_day", DEFAULT_SETUP_SCHEDULE.open_day],
    ["schedule_open_time", DEFAULT_SETUP_SCHEDULE.open_time],
    ["schedule_end_day", DEFAULT_SETUP_SCHEDULE.nominal_end_day],
    ["schedule_end_time", DEFAULT_SETUP_SCHEDULE.nominal_end_time],
  ] as const) {
    draft = setupDraftSet(draft, field, value);
  }
  return draft;
}

/** 读取字段的原始输入文本；数字也保持在用户输入时的表示。 */
export function setupDraftValue(draft: SetupDraft, field: SetupField): string {
  return draft.values[field];
}

/** 写回单个字段；数据库目标变化时立即撤销旧确认。 */
export function setupDraftSet(draft: SetupDraft, field: SetupField, value: string): SetupDraft {
  const previous = draft.values[field];
  // 授权只绑定到当时的目标。改后再改回也必须重新勾选。
  const confirmedDatabaseTarget =
    SETUP_FIELD_DATABASE_TARGET.includes(field) && previous !== value
      ? null
      : draft.confirmedDatabaseTarget;
  return {
    ...draft,
    values: { ...draft.values, [field]: value },
    confirmedDatabaseTarget,
  };
}

/** 两个独立 v4 UUID 共含 244 位随机熵；使用既有系统/浏览器安全随机源，不降级。 */
export function setupDraftGenerateAdminSecret(draft: SetupDraft): SetupDraft {
  const secret = `${createUuidV4().replaceAll("-", "")}${createUuidV4().replaceAll("-", "")}`;
  const withSecret = setupDraftSet(draft, "admin_secret", secret);
  return setupDraftSet(withSecret, "admin_confirmation", secret);
}

/** 只恢复性能与日志草稿；凭据、站点、监听端口和业务开关不受影响。 */
export function setupDraftResetAdvanced(draft: SetupDraft): SetupDraft {
  const defaults = createSetupDraft();
  let reset = draft;
  for (const field of SETUP_FIELD_ADVANCED) {
    reset = setupDraftSet(reset, field, setupDraftValue(defaults, field));
  }
  return reset;
}

/** 结束当前设置会话时清空所有密码和已确认的目标。 */
export function setupDraftClearPasswords(draft: SetupDraft): SetupDraft {
  return {
    ...draft,
    values: {
      ...draft.values,
      database_password: "",
      admin_secret: "",
      admin_confirmation: "",
    },
    confirmedDatabaseTarget: null,
  };
}
