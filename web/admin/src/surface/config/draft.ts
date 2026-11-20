// 当前管理会话内的非敏感配置草稿；不使用浏览器持久化存储。

import type { AdminJsonValue } from "@auction/contracts/dto/admin/rules";
import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import { type Accessor, createContext } from "solid-js";

import { adminApiFailureIsConflict } from "../../api/failure";
import { type AdminRuntimeConfig, validateAdminRuntimeConfig } from "../../api/runtime-config";
import type { AdminSessionHost } from "../../api/session";
import { advanceCommandReload, createCommandReload } from "../pending/sources";
import { useUnloadWarning } from "../presentation";
import {
  type RuntimeConfigForm,
  runtimeConfigFormDefault,
  runtimeConfigFormRefreshEquipmentTaxonomy,
  runtimeConfigFormSameEditsAs,
} from "./model";
import type { ConfigCommand, ConfigNotice } from "./page";

/**
 * 草稿由认证外壳持有，站内切页不丢失输入与重试命令；退出当前会话后随外壳销毁。
 */
export interface ConfigDraft {
  readonly form: Cell<RuntimeConfigForm>;
  readonly baseline: Cell<RuntimeConfigForm | null>;
  readonly fingerprint: Cell<string | null>;
  readonly checkedSignature: Cell<string>;
  readonly command: Cell<ConfigCommand | null>;
  readonly notice: Cell<ConfigNotice | null>;
  readonly restartRequired: Cell<boolean>;
  readonly dirty: Accessor<boolean>;
  readonly refresh: Cell<bigint>;
  readonly validating: Cell<boolean>;

  /** 在认证会话中预校验候选。回执只适用于发出时的基线和草稿内容。 */
  validate(
    host: AdminSessionHost,
    fingerprint: string,
    document: AdminJsonValue,
    signature: string,
  ): void;

  /** 只允许增加重启提醒；Worker 冷配置状态不能清除通用提醒。 */
  rememberRestartNotice(required: boolean): void;

  /** 写入成功后替换基线、版本围栏与草稿编辑内容。 */
  applySavedConfig(
    result: AdminRuntimeConfig,
    nextForm: RuntimeConfigForm,
    requiresRestart: boolean,
  ): void;
}

/** 认证外壳向配置页提供的草稿；缺少提供方是装配错误。 */
export const ConfigDraftContext = createContext<ConfigDraft | null>(null);

/**
 * 安装一次草稿上下文；浏览器整页离开时由原生提示保护尚未保存的输入。
 * Provider 由认证外壳使用 `ConfigDraftContext` 包裹子树。
 */
export function useConfigDraft(): ConfigDraft {
  const form = createCell(runtimeConfigFormDefault());
  const baseline = createCell<RuntimeConfigForm | null>(null);
  const fingerprint = createCell<string | null>(null);
  const checkedSignature = createCell("");
  const command = createCell<ConfigCommand | null>(null);
  const notice = createCell<ConfigNotice | null>(null);
  const restartRequired = createCell(false);
  const refresh = createCommandReload(0n);
  const validating = createCell(false);
  const tasks = createTaskScope();
  const dirty = createValueMemo(() => {
    const initial = baseline();
    return initial !== null && !runtimeConfigFormSameEditsAs(initial, form());
  });

  const rememberRestartNotice = (required: boolean): void => {
    // 通用重启提醒涵盖所有受影响的进程，Worker 冷配置状态不能清除它。
    if (required) {
      restartRequired.set(true);
    }
  };

  const draft: ConfigDraft = {
    form,
    baseline,
    fingerprint,
    checkedSignature,
    command,
    notice,
    restartRequired,
    dirty,
    refresh,
    validating,

    validate(host, validationFingerprint, document, signature) {
      const candidate = copyRuntimeConfigForm(form.peek());
      if (candidate === null) {
        return;
      }
      validating.set(true);
      tasks.spawn(async (context) => {
        const result = await validateAdminRuntimeConfig(host, document, context.signal);
        if (!context.alive()) {
          return;
        }
        // 配置页可能已经卸载；只有会话基线及原始编辑仍相同时才能采用回执。
        const sameBaseline = fingerprint.peek() === validationFingerprint;
        if (sameBaseline && runtimeConfigFormSameEditsAs(form.peek(), candidate)) {
          let nextNotice: ConfigNotice;
          if (result.ok) {
            const taxonomy = result.value.equipment_taxonomy;
            if (taxonomy !== null) {
              form.update((current) =>
                runtimeConfigFormRefreshEquipmentTaxonomy(current, taxonomy),
              );
              checkedSignature.set(signature);
              command.set(null);
              nextNotice = { kind: "validated" };
            } else {
              nextNotice = { kind: "invalid-form" };
            }
          } else if (adminApiFailureIsConflict(result.error)) {
            nextNotice = { kind: "conflict" };
          } else {
            nextNotice = { kind: "action-error", code: result.error.code };
          }
          notice.set(nextNotice);
        }
        validating.set(false);
      });
    },

    rememberRestartNotice,

    applySavedConfig(result, nextForm, requiresRestart) {
      // 只在写入成功后替换基线和版本围栏，同时解除本次命令及校验签名。
      baseline.set(copyRuntimeConfigForm(nextForm) ?? nextForm);
      form.set(nextForm);
      checkedSignature.set("");
      fingerprint.set(result.fingerprint);
      // 写入响应采用通用重启标志。实际差异决定本次是否增加提醒，既有提醒继续保留。
      rememberRestartNotice(result.restart_required && requiresRestart);
      command.set(null);
      advanceCommandReload(refresh);
    },
  };

  useUnloadWarning(dirty);
  return draft;
}

/**
 * 冻结校验发出时的编辑内容。`RuntimeConfigForm` 是 JSON 形状的数据；若装配方提供了
 * 不可克隆的值，不把异常交给整页失败路径，也不采用无法可靠比较的回执。
 */
function copyRuntimeConfigForm(form: RuntimeConfigForm): RuntimeConfigForm | null {
  try {
    return structuredClone(form);
  } catch {
    return null;
  }
}
