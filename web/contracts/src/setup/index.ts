// 仅用于本机首次部署的初始化请求与稳定结果码；正式 Admin 进程不挂载这些路由。

import * as v from "valibot";
import { struct, withDefault } from "../wire";

/** 一次性初始化的数据库输入；与配置模型分离，不返回到响应或常规管理 DTO。 */
export interface SetupDatabaseRequest {
  host: string;
  port: number;
  username: string;
  password: string;
  name: string;
}

/** 首次部署的本地周排期；不接受 world 或 runtime 接管参数。
 * 容器级 `serde(default)` 用默认值补齐缺项，而不是使用空字符串。 */
export interface SetupScheduleRequest {
  /** IANA 时区名称；下方本地时间不按浏览器时区解释。 */
  timezone: string;
  preview_day: string;
  /** 预览本地时间，必须为精确 HH:MM。 */
  preview_time: string;
  /** 同一周内的开拍星期，使用英文完整名称。 */
  open_day: string;
  /** 开拍本地时间；必须至少比预览晚 120 分钟。 */
  open_time: string;
  /** 同一周内的名义结束星期，使用英文完整名称。 */
  nominal_end_day: string;
  /** 名义结束本地时间，必须晚于开拍；不改变软截止规则。 */
  nominal_end_time: string;
}

/** `SetupScheduleRequest` 的 Rust `Default` 值；容器缺省时排期字段沿用这些值。 */
export const DEFAULT_SETUP_SCHEDULE: Readonly<SetupScheduleRequest> = {
  timezone: "UTC",
  preview_day: "Thursday",
  preview_time: "22:00",
  open_day: "Friday",
  open_time: "00:00",
  nominal_end_day: "Saturday",
  nominal_end_time: "12:00",
};

/** Public 回环监听端口的缺省值。 */
export const DEFAULT_PUBLIC_LISTENER_PORT = 8090;

/** Admin 回环监听端口的缺省值。 */
export const DEFAULT_ADMIN_LISTENER_PORT = 8091;

/** 一次性初始化的表单内容；数据库凭据和管理员密码只能出现在请求及目标敏感 TOML 中。 */
export interface SetupRequest {
  database: SetupDatabaseRequest;
  admin_actor: string;
  admin_secret: string;
  public_origin: string;
  admin_origin: string;
  /** 正式 Public 服务的回环 TCP 端口；省略时使用 8090。 */
  public_listener_port?: number;
  /** 正式 Admin 服务的回环 TCP 端口；省略时使用 8091，不能与 Public 相同。 */
  admin_listener_port?: number;
  metrics_enabled: boolean;
  schedule_enabled: boolean;
  /** 排期关闭时仍保存并校验候选；启用开关继续使用既有顶层字段。容器缺省时沿用默认排期。 */
  schedule?: SetupScheduleRequest;
  start_price: number;
  log_filter: string;
  worker_concurrency: number;
  worker_poll_interval_seconds: number;
  realtime_poll_interval_millis: number;
  /** 合并入口明确确认在配置保存后初始化专用空库并启动；旧分体请求可省略。 */
  confirm_initialize?: boolean;
}

/** 初始化接口的成功阶段：`verified` 仅验证部署码，`validated` 检查输入和数据库连接，
 * `completed` 表示写入完成且初始化进程退出；验证响应不授予后续请求的免鉴权资格。 */
export const SetupResponseSchema = struct({
  status: v.string(),
  /** 当前初始化表面是否在保存后由 supervisor 初始化并启动；不表示服务已经就绪。 */
  initialize_and_start: withDefault(v.boolean(), false),
});

export type SetupResponse = v.InferOutput<typeof SetupResponseSchema>;

/** 不包含底层数据库、TOML 或文件内容的失败响应。 */
export const SetupErrorResponseSchema = struct({
  code: v.string(),
});

export type SetupErrorResponse = v.InferOutput<typeof SetupErrorResponseSchema>;
