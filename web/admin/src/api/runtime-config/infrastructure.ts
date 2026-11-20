// 运行配置基础设施区块的响应结构校验。

import { fieldValue, validScalars, validStringArray } from "./fields";

/** 校验监听地址区块的闭集字段与监听模式。 */
export function validListener(value: unknown): boolean {
  return (
    validScalars(
      value,
      [],
      ["address", "group", "mode", "runtime_directory", "socket_path"],
      [],
      [],
    ) && isListenerMode(fieldValue(value, "mode"))
  );
}

/** 校验指标区块只含布尔开关。 */
export function validMetrics(value: unknown): boolean {
  return validScalars(value, ["enabled"], [], [], []);
}

/** 校验安全区块的限流字段与来源列表。 */
export function validSecurity(value: unknown): boolean {
  return (
    validScalars(
      value,
      ["allowed_origins_enabled", "cookie_secure", "trusted_unix_proxy"],
      [],
      [
        "login_rate_limit",
        "mutation_rate_limit",
        "rate_limit_window_seconds",
        "request_timeout_seconds",
      ],
      ["allowed_origins", "trusted_proxy_cidrs"],
    ) &&
    validStringArray(fieldValue(value, "allowed_origins")) &&
    validStringArray(fieldValue(value, "trusted_proxy_cidrs"))
  );
}

/** 校验会话区块的三个非负整数字段。 */
export function validSession(value: unknown): boolean {
  return validScalars(
    value,
    [],
    [],
    ["absolute_ttl_seconds", "idle_ttl_seconds", "max_public_sessions"],
    [],
  );
}

/** 校验追踪区块的端点、采样与导出限制字段。 */
export function validTracing(value: unknown): boolean {
  return validScalars(
    value,
    ["enabled"],
    ["endpoint"],
    [
      "batch_timeout_seconds",
      "export_timeout_seconds",
      "max_export_batch_size",
      "queue_capacity",
      "sample_ratio_ppm",
      "shutdown_timeout_seconds",
    ],
    [],
  );
}

/** 校验 Worker 控制区块的 socket 路径字段。 */
export function validWorkerControl(value: unknown): boolean {
  return validScalars(value, [], ["socket_path"], [], []);
}

function isListenerMode(value: unknown): boolean {
  return value === "tcp" || value === "unix";
}
