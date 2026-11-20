// Admin 邮件诊断的可选扩展；仅包含经过筛选的数字 MID 和运行观测。

import * as v from "valibot";
import { optionalNullable, strictStruct } from "../../../wire";

/** 最近一次失败定位；Worker 重启后清空，不能据此自动跳过或重发。 */
export const AdminMailPollProgressFailureResponseSchema = strictStruct({
  stage: v.string(),
  external_mail_id: optionalNullable(v.string()),
  occurred_at: v.string(),
});

export type AdminMailPollProgressFailureResponse = v.InferOutput<
  typeof AdminMailPollProgressFailureResponseSchema
>;

/** 进程内入站阶段；刷新时读取，不代表外部发送或附件动作完成。 */
export const AdminMailPollProgressResponseSchema = strictStruct({
  mailbox_key: v.string(),
  world_id: v.string(),
  stage: v.string(),
  external_mail_id: optionalNullable(v.string()),
  updated_at: v.string(),
  last_failure: optionalNullable(AdminMailPollProgressFailureResponseSchema),
});

export type AdminMailPollProgressResponse = v.InferOutput<
  typeof AdminMailPollProgressResponseSchema
>;

/** 通过显式查询选择的诊断，不含原始 cursor、claim token、发件人、正文或凭据。 */
export const AdminMailPollingDiagnosticsResponseSchema = strictStruct({
  observed_at: v.string(),
  /** available / empty / hidden；hidden 不回传原始值。 */
  cursor_status: v.string(),
  completed_external_mail_id: optionalNullable(v.string()),
  /** available / unavailable / timeout / incompatible / rejected。 */
  worker_condition: v.string(),
  worker_observed_at: optionalNullable(v.string()),
  progress: optionalNullable(AdminMailPollProgressResponseSchema),
});

export type AdminMailPollingDiagnosticsResponse = v.InferOutput<
  typeof AdminMailPollingDiagnosticsResponseSchema
>;
