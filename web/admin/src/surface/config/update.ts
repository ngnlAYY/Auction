// 运行配置写入命令的冻结执行体；成功结果同时投影出下一份安全表单。

import type { AdminRuntimeConfigResponse } from "@auction/contracts/dto/admin/rules";
import { err, ok, type Result } from "@auction/web-shared/result";
import { type AdminWriteFailure, writeOutcomeUnknown } from "../../api/failure";
import {
  type AdminRuntimeConfig,
  type AdminRuntimeConfigCommandPreparation,
  updateAdminRuntimeConfig,
} from "../../api/runtime-config";
import type { AdminSessionHost } from "../../api/session";
import { type RuntimeConfigForm, runtimeConfigFormFromResponse } from "./model";

export interface ConfigUpdateOutcome {
  readonly result: AdminRuntimeConfig;
  readonly nextForm: RuntimeConfigForm;
}

/** 每次重试复用同一个准备好的命令正文，校验失败视为写入结果未知。 */
export async function updateRuntimeConfigCommand(
  host: AdminSessionHost,
  commandId: string,
  prepared: AdminRuntimeConfigCommandPreparation,
  signal: AbortSignal,
): Promise<Result<ConfigUpdateOutcome, AdminWriteFailure>> {
  const result = await updateAdminRuntimeConfig(host, commandId, prepared, signal);
  if (!result.ok) {
    return result;
  }
  const nextForm = runtimeConfigFormFromResponse(responseFromApi(result.value));
  if (!nextForm.ok) {
    return err(writeOutcomeUnknown("invalid_response"));
  }
  return ok({ result: result.value, nextForm: nextForm.value });
}

/** 写命令结果回到安全表单投影；词典缺失由 from_response 判为非法响应。 */
function responseFromApi(value: AdminRuntimeConfig): AdminRuntimeConfigResponse {
  return {
    document: value.document,
    fingerprint: value.fingerprint,
    restart_required: value.restart_required,
    equipment_taxonomy: value.equipment_taxonomy,
  };
}
