// 运行配置读取世代与表单快照装配。

import type {
  AdminRuntimeConfigResponse,
  AdminWorkerRestartStateDto,
} from "@auction/contracts/dto/admin/rules";
import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { err } from "@auction/web-shared/result";
import { createEffect } from "solid-js";

import { adminApiFailureClient } from "../../api/failure";
import { type AdminRuntimeConfig, fetchAdminRuntimeConfig } from "../../api/runtime-config";
import type { AdminSessionHost } from "../../api/session";
import type { PendingCommands } from "../pending";
import type { ConfigDraft } from "./draft";
import {
  type RuntimeConfigForm,
  runtimeConfigFormFromResponse,
  runtimeConfigFormRefreshEquipmentTaxonomy,
  runtimeConfigFormSameEditsAs,
} from "./model";
import type { FieldIssue } from "./validation";

/** 读取完成后的投影处理结果。 */
export type ConfigReadStatus =
  | { readonly kind: "ready"; readonly fingerprint: string }
  | { readonly kind: "conflict" }
  | { readonly kind: "invalid-form" }
  | { readonly kind: "error"; readonly code: string };

/** 本轮读取已处理，可据此显示状态或校验写入基线。 */
export interface CompletedConfigRead {
  readonly generation: bigint;
  readonly status: ConfigReadStatus;
  readonly restartRequired: boolean;
  readonly workerRestartState: AdminWorkerRestartStateDto | null;
}

/** 只有本轮读取接受的基线指纹能授权配置操作。 */
export function completedConfigReadMatchesBaseline(
  read: CompletedConfigRead,
  generation: bigint,
  fingerprint: string,
): boolean {
  return (
    read.generation === generation &&
    read.status.kind === "ready" &&
    read.status.fingerprint === fingerprint
  );
}

/**
 * 记录本次挂载是否已有编辑器，以及已处理的读取结果。新读取世代在结果处理完前不能发起操作。
 */
export function useConfigRead(
  host: AdminSessionHost,
  draft: ConfigDraft,
  pending: PendingCommands,
  issues: Cell<readonly FieldIssue[]>,
): readonly [Cell<boolean>, Cell<CompletedConfigRead | null>] {
  const editorMounted = createCell(false);
  const completed = createCell<CompletedConfigRead | null>(null);
  const config = createTaskResource(
    () => draft.refresh(),
    async (generation, context) => {
      try {
        const result = await fetchAdminRuntimeConfig(host, context.signal);
        return { generation, result };
      } catch {
        // 等待后的装配异常只按固定错误码处理，异常对象与原文不进入页面或控制台。
        return { generation, result: err(adminApiFailureClient("network_error")) };
      }
    },
  );

  createEffect(
    () => ({
      generation: draft.refresh(),
      state: config.state(),
      snapshot: config(),
      pendingRuntimeConfig: runtimeConfigPending(draft, pending),
    }),
    ({ generation, state, snapshot, pendingRuntimeConfig }) => {
      if (state !== "ready" || snapshot === undefined) {
        return;
      }
      // Resource 在挂起期间保留旧值。只准本轮结果装配草稿及开放操作。
      if (snapshot.generation !== generation) {
        return;
      }
      let status: ConfigReadStatus;
      let restartRequired: boolean;
      let workerRestartState: AdminWorkerRestartStateDto | null;
      if (snapshot.result.ok) {
        const value = snapshot.result.value;
        status = applyConfigRead(draft, issues, value, pendingRuntimeConfig);
        restartRequired = value.restart_required;
        workerRestartState = value.worker_restart_state;
      } else {
        // 失败不能替换最近一次成功读取的 Worker 状态。
        status = { kind: "error", code: snapshot.result.error.code };
        restartRequired = false;
        workerRestartState = completed.peek()?.workerRestartState ?? null;
      }
      if (status.kind === "ready" || status.kind === "conflict") {
        editorMounted.set(true);
      }
      completed.set({ generation, status, restartRequired, workerRestartState });
    },
  );

  return [editorMounted, completed];
}

/** compute 阶段读取本实体的未决状态；effect 的 apply 不追踪依赖。 */
function runtimeConfigPending(draft: ConfigDraft, pending: PendingCommands): boolean {
  return (
    draft.baseline.peek() !== null &&
    draft.command.peek() !== null &&
    pending.contains("runtime-config")
  );
}

function applyConfigRead(
  draft: ConfigDraft,
  issues: Cell<readonly FieldIssue[]>,
  value: AdminRuntimeConfig,
  pendingRuntimeConfig: boolean,
): ConfigReadStatus {
  const matchesBaseline = draft.fingerprint.peek() === value.fingerprint;
  const hasBaseline = draft.baseline.peek() !== null;
  // 未决命令保留提交时的基线、指纹和冻结负载，直到原命令了结。
  if (hasBaseline && draft.command.peek() !== null && pendingRuntimeConfig) {
    return matchesBaseline
      ? { kind: "ready", fingerprint: value.fingerprint }
      : { kind: "conflict" };
  }
  const taxonomy = value.equipment_taxonomy;
  if (taxonomy === null) {
    return { kind: "invalid-form" };
  }
  if (hasBaseline) {
    const initial = draft.baseline.peek();
    const dirty = initial !== null && !runtimeConfigFormSameEditsAs(initial, draft.form.peek());
    if (matchesBaseline || dirty) {
      // 词典独立于文档指纹演进。保持表单与基线的原始值及编辑记录。
      draft.form.update((current) => runtimeConfigFormRefreshEquipmentTaxonomy(current, taxonomy));
      draft.baseline.update((current) =>
        current === null ? null : runtimeConfigFormRefreshEquipmentTaxonomy(current, taxonomy),
      );
      if (matchesBaseline) {
        return { kind: "ready", fingerprint: value.fingerprint };
      }
      // 已观察到基线变化，旧预校验不能授权写入，即使以后又读到原指纹。
      draft.checkedSignature.set("");
      return { kind: "conflict" };
    }
  }
  const nextForm = runtimeConfigFormFromResponse(responseFromApi(value));
  if (!nextForm.ok) {
    return { kind: "invalid-form" };
  }
  const baselineForm = copyRuntimeConfigForm(nextForm.value);
  if (baselineForm === null) {
    return { kind: "invalid-form" };
  }
  draft.baseline.set(baselineForm);
  draft.form.set(nextForm.value);
  draft.fingerprint.set(value.fingerprint);
  draft.rememberRestartNotice(value.restart_required);
  draft.checkedSignature.set("");
  draft.command.set(null);
  draft.notice.set(null);
  issues.set([]);
  return { kind: "ready", fingerprint: value.fingerprint };
}

function responseFromApi(value: AdminRuntimeConfig): AdminRuntimeConfigResponse {
  return {
    document: value.document,
    fingerprint: value.fingerprint,
    restart_required: value.restart_required,
    equipment_taxonomy: value.equipment_taxonomy,
  };
}

/** 基线与表单一同替换时保留独立快照；克隆失败按响应损坏拒绝，保留原草稿与基线。 */
function copyRuntimeConfigForm(form: RuntimeConfigForm): RuntimeConfigForm | null {
  try {
    return structuredClone(form);
  } catch {
    return null;
  }
}
