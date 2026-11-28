// 运行配置文档编辑器：读取草稿、服务端校验、确认写入与变更摘要。

import type { AdminWorkerRestartStateDto } from "@auction/contracts/dto/admin/rules";
import { createCell } from "@auction/web-shared/reactive/cell";
import { Dialog } from "@auction/web-shared/ui/components/dialog";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, createMemo, For, onCleanup, Show, useContext } from "solid-js";

import { prepareAdminRuntimeConfigUpdateOnce } from "../../api/runtime-config";
import { useAdminSessionHost } from "../auth/shell";
import { commandDeliveryReported, PendingCommandsContext, setIfAlive } from "../pending";
import { PendingCommandPanel } from "../pending/panel";
import { advanceCommandReload, usePendingCommands } from "../pending/sources";
import { adminApiErrorMessage } from "../presentation";
import { AdminRuntimeConfigFact, workerRestartMessage } from "./document-facts";
import { type ConfigDraft, ConfigDraftContext } from "./draft";
import { RuntimeConfigFields } from "./form";
import {
  runtimeConfigFormBoolean,
  runtimeConfigFormSignature,
  runtimeConfigFormSourceDocument,
} from "./model";
import { runtimeConfigFormToDocument } from "./model/document";
import { type ConfigNotice, noticeText } from "./notice";
import { type ReadFeedback, ReadFeedbackPanel } from "./read-panel";
import {
  type CompletedConfigRead,
  type ConfigReadStatus,
  completedConfigReadMatchesBaseline,
  useConfigRead,
} from "./reading";
import {
  type ConfigChange,
  ConfigChangeList,
  configChangeNeedsRestart,
  configChanges as reviewChanges,
} from "./review";
import { type ConfigUpdateOutcome, updateRuntimeConfigCommand } from "./update";
import {
  collectIssues,
  ErrorSummary,
  type FieldIssue,
  FormIssuesContext,
  focusSummary,
} from "./validation";

/** 渲染时的配置基线快照；事件入口用它与最新状态核对。 */
interface ConfigBaselineSnapshot {
  readonly generation: bigint;
  readonly fingerprint: string;
  readonly matchesBaseline: boolean;
}

export function RuntimeConfigDocument(): JSX.Element {
  const host = useAdminSessionHost();
  const configDraft = requireConfigDraft();
  const {
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
  } = configDraft;
  const pending = usePendingCommands(refresh, (entity) => entity === "runtime-config");
  const issues = createCell<readonly FieldIssue[]>([]);
  const busy = () => validating() || pending.isBusy("runtime-config");
  const frozen = () => validating() || pending.contains("runtime-config");
  let pageAlive = true;
  onCleanup(() => {
    pageAlive = false;
  });
  // 写入是危险操作：点击「写入配置」只准备命令并打开展示变更摘要的确认对话框。
  const updateConfirmOpen = createCell(false);
  const reloadConfirmOpen = createCell(false);
  const [editorMounted, completedRead] = useConfigRead(host, configDraft, pending, issues);

  const discardAndReload = (): void => {
    if (frozen()) {
      return;
    }
    baseline.set(null);
    fingerprint.set(null);
    checkedSignature.set("");
    command.set(null);
    notice.set(null);
    issues.set([]);
    reloadConfirmOpen.set(false);
    advanceCommandReload(refresh);
  };

  const currentRead = createMemo((): CompletedConfigRead | null => {
    const generation = refresh();
    const read = completedRead();
    return read !== null && read.generation === generation ? read : null;
  });
  const readStatus = createMemo((): ConfigReadStatus | null => currentRead()?.status ?? null);
  // 渲染时的读取世代与指纹；事件入口再用 peek() 核对，避免在响应式更新提交前
  // 继续使用已过期的基线。
  const baselineSnapshot = createMemo((): ConfigBaselineSnapshot | null => {
    const generation = refresh();
    const currentFingerprint = fingerprint();
    const read = completedRead();
    if (currentFingerprint === null || read === null || !editorMounted()) {
      return null;
    }
    return {
      generation,
      fingerprint: currentFingerprint,
      matchesBaseline: completedConfigReadMatchesBaseline(read, generation, currentFingerprint),
    };
  });
  const matchesBaseline = createMemo(() => baselineSnapshot()?.matchesBaseline ?? false);
  // effect 的 apply 在响应式更新提交后保存这次渲染的快照；事件处理器只读取已提交的值。
  const renderedBaseline = createCell<ConfigBaselineSnapshot | null>(null);
  createEffect(baselineSnapshot, (snapshot) => {
    renderedBaseline.set(snapshot);
  });
  const currentBaseline = (): ConfigBaselineSnapshot | null => {
    const snapshot = renderedBaseline.peek();
    if (snapshot === null || !snapshot.matchesBaseline) {
      return null;
    }
    const read = completedRead.peek();
    return refresh.peek() === snapshot.generation &&
      fingerprint.peek() === snapshot.fingerprint &&
      read !== null &&
      completedConfigReadMatchesBaseline(read, snapshot.generation, snapshot.fingerprint)
      ? snapshot
      : null;
  };
  const readRequiresRestart = createMemo(() => currentRead()?.restartRequired ?? false);
  // 失败或新读取挂起时继续展示最近一次成功读取的 Worker 状态。
  const workerRestartState = createMemo(
    (): AdminWorkerRestartStateDto | null => completedRead()?.workerRestartState ?? null,
  );
  const readFeedback = createMemo((): ReadFeedback | null => {
    const status = readStatus();
    if (status === null) {
      return { kind: "loading", title: t("admin-runtime-loading") };
    }
    if (status.kind === "ready") {
      return null;
    }
    if (status.kind === "conflict") {
      return { kind: "conflict", title: noticeText({ kind: "conflict" }) };
    }
    if (status.kind === "error") {
      return { kind: "error", title: adminApiErrorMessage(status.code) };
    }
    return { kind: "error", title: t("admin-runtime-invalid-form") };
  });
  const canUpdate = createMemo(() => {
    if (!matchesBaseline() || frozen() || !dirty()) {
      return false;
    }
    const signature = checkedSignature();
    if (signature === "") {
      return false;
    }
    const current = runtimeConfigFormSignature(form());
    return current.ok && current.value === signature;
  });
  const enabledWorlds = createMemo(
    () =>
      ["main", "isekai"].filter((world) =>
        runtimeConfigFormBoolean(form(), `worlds.${world}.enabled`),
      ).length,
  );
  const equipmentFilterEnabled = createMemo(() =>
    runtimeConfigFormBoolean(form(), "equipment_filter.enabled"),
  );
  const canConfirm = createMemo(() => {
    if (!matchesBaseline() || !updateConfirmOpen() || checkedSignature() === "") {
      return false;
    }
    const pendingCommand = command();
    return pendingCommand !== null && pendingCommand.fingerprint === fingerprint();
  });
  const configChanges = createMemo((): readonly ConfigChange[] => {
    if (!updateConfirmOpen()) {
      return [];
    }
    const pendingCommand = command();
    const original = baseline();
    if (pendingCommand === null || original === null) {
      return [];
    }
    return reviewChanges(runtimeConfigFormSourceDocument(original), pendingCommand.document);
  });
  const requiresRestart = createMemo(() =>
    configChanges().some((change) => configChangeNeedsRestart(change)),
  );
  const confirmationReadFeedback = createMemo(() => {
    const feedback = readFeedback();
    if (feedback !== null) {
      return feedback.title;
    }
    if (canConfirm()) {
      return "";
    }
    return noticeText({ kind: "conflict" });
  });
  const currentNotice = createMemo((): ConfigNotice | null => {
    const value = notice();
    if (value === null) {
      return null;
    }
    if (value.kind === "saved") {
      return null;
    }
    if (value.kind === "validated") {
      return matchesBaseline() && checkedSignature() !== "" ? value : null;
    }
    if (value.kind === "conflict") {
      return readStatus()?.kind === "conflict" ? null : value;
    }
    return value;
  });
  const visibleFingerprint = createMemo(() => (editorMounted() ? fingerprint() : null));

  const onSubmit = (event: SubmitEvent): void => {
    event.preventDefault();
    const baseline = currentBaseline();
    if (frozen() || baseline === null) {
      return;
    }
    checkedSignature.set("");
    notice.set(null);
    const currentForm = form();
    const fieldIssues = collectIssues(currentForm);
    if (fieldIssues.length > 0) {
      issues.set(fieldIssues);
      focusSummary();
      return;
    }
    issues.set([]);
    const document = runtimeConfigFormToDocument(currentForm);
    if (!document.ok) {
      notice.set({ kind: "invalid-form" });
      return;
    }
    const signature = runtimeConfigFormSignature(currentForm);
    if (!signature.ok) {
      notice.set({ kind: "invalid-form" });
      return;
    }
    configDraft.validate(host, baseline.fingerprint, document.value, signature.value);
  };

  const onDirty = (): void => {
    if (frozen()) {
      return;
    }
    checkedSignature.set("");
    command.set(null);
    notice.set(null);
    if (issues.peek().length > 0) {
      // 输入可以纠正已报告的错误，新错误等离焦或再次提交。
      const previous = issues.peek();
      const remaining = collectIssues(form.peek()).filter((issue: FieldIssue) =>
        previous.some((old: FieldIssue) => old.path === issue.path),
      );
      issues.set(remaining);
    }
  };

  const onUpdate = (): void => {
    const baseline = currentBaseline();
    if (frozen() || baseline === null) {
      return;
    }
    const document = runtimeConfigFormToDocument(form.peek());
    if (!document.ok) {
      notice.set({ kind: "invalid-form" });
      return;
    }
    const signature = runtimeConfigFormSignature(form.peek());
    if (!signature.ok || signature.value !== checkedSignature.peek()) {
      return;
    }
    command.set({ fingerprint: baseline.fingerprint, document: document.value });
    notice.set(null);
    updateConfirmOpen.set(true);
  };

  const onConfirmUpdate = (): void => {
    if (
      frozen() ||
      !updateConfirmOpen() ||
      currentBaseline() === null ||
      checkedSignature.peek() === ""
    ) {
      return;
    }
    const currentCommand = command.peek();
    if (currentCommand === null) {
      updateConfirmOpen.set(false);
      return;
    }
    if (fingerprint.peek() !== currentCommand.fingerprint) {
      return;
    }
    // 冻结确认那一刻的版本围栏、负载与重启判定；重试只复用这些值。
    const prepared = prepareAdminRuntimeConfigUpdateOnce(
      currentCommand.fingerprint,
      currentCommand.document,
    );
    const requiresRestartForReceipt = requiresRestart();
    const begun = pending.submitWithReceipt<ConfigUpdateOutcome>(
      "runtime-config",
      t("admin-runtime-update"),
      (commandId, signal) => updateRuntimeConfigCommand(host, commandId, prepared, signal),
      (value, sourceAlive) => {
        const restart = value.result.restart_required && requiresRestartForReceipt;
        // 写入回执同时推进会话草稿的版本与读取世代。
        configDraft.applySavedConfig(value.result, value.nextForm, requiresRestartForReceipt);
        if (sourceAlive) {
          setIfAlive(notice, { kind: "saved" }, () => pageAlive);
        }
        return commandDeliveryReported(sourceAlive, () =>
          restart ? { kind: "text" as const, text: t("admin-runtime-restart-required") } : null,
        );
      },
      (code) => {
        setIfAlive(notice, { kind: "action-error", code }, () => pageAlive);
      },
    );
    // 宿主未受理（会话不再允许写入或同一实体已有命令）时保留确认框与已冻结的候选。
    if (!begun) {
      return;
    }
    updateConfirmOpen.set(false);
    notice.set(null);
  };

  return (
    <PendingCommandsContext value={pending}>
      <FormIssuesContext value={issues}>
        <h2 class="admin-config-page__runtime-title">{t("admin-runtime-document-title")}</h2>
        <div role="status">
          <Show when={workerRestartState()}>
            {(state) => (
              <p
                class={
                  state() === "restart_required"
                    ? "admin-panel__note admin-runtime-config__restart"
                    : "admin-panel__note"
                }
              >
                {workerRestartMessage(state())}
              </p>
            )}
          </Show>
        </div>
        <PendingCommandPanel owner={pending} />
        <div role="status">
          <Show when={notice()?.kind === "saved"}>
            <p class="admin-panel__note action-feedback action-feedback--success">
              {noticeText({ kind: "saved" })}
            </p>
          </Show>
          <Show when={restartRequired() || readRequiresRestart()}>
            <p class="admin-panel__note admin-runtime-config__restart">
              {t("admin-runtime-restart-required")}
            </p>
          </Show>
        </div>
        <Show when={visibleFingerprint()}>
          {(currentFingerprint) => (
            <div class="admin-runtime-config__content" aria-busy={busy() ? "true" : "false"}>
              <div class="admin-config-facts">
                <AdminRuntimeConfigFact
                  detail={`${enabledWorlds()}/2`}
                  state={enabledWorlds() > 0}
                  label={t("admin-runtime-enabled-worlds")}
                />
                <AdminRuntimeConfigFact
                  detail={
                    equipmentFilterEnabled()
                      ? t("admin-runtime-enabled")
                      : t("admin-runtime-disabled")
                  }
                  state={equipmentFilterEnabled()}
                  label={t("admin-runtime-equipment-filter")}
                />
                <AdminRuntimeConfigFact
                  detail={currentFingerprint()}
                  state={null}
                  label={t("admin-runtime-fingerprint-label")}
                />
              </div>
              <form
                class="admin-runtime-config__form admin-config-form"
                novalidate={true}
                onSubmit={onSubmit}
              >
                <Show when={dirty()}>
                  <p class="admin-config-draft" role="status">
                    {t("admin-runtime-unsaved")}
                  </p>
                </Show>
                <Show when={issues().length > 0}>
                  <ErrorSummary issues={issues()} />
                </Show>
                {/* 以已接受的快照指纹重建字段控件，避免继承上一份配置的本地编辑与播报状态。 */}
                <For each={[currentFingerprint()]} keyed={(key) => key}>
                  {() => <RuntimeConfigFields form={form} disabled={frozen()} onDirty={onDirty} />}
                </For>
                <Show when={currentNotice()}>
                  {(current) => (
                    <p
                      class={
                        current().kind === "validated"
                          ? "admin-panel__note action-feedback action-feedback--success"
                          : "admin-panel__note action-feedback action-feedback--error"
                      }
                      role={current().kind === "validated" ? "status" : "alert"}
                    >
                      {noticeText(current())}
                    </p>
                  )}
                </Show>
                <div class="admin-panel__actions">
                  <button
                    class="admin-btn-secondary"
                    type="button"
                    disabled={frozen()}
                    onClick={() => {
                      if (frozen()) {
                        return;
                      }
                      if (dirty()) {
                        reloadConfirmOpen.set(true);
                      } else {
                        advanceCommandReload(refresh);
                      }
                    }}
                  >
                    {t("admin-runtime-reload")}
                  </button>
                  <button
                    class="admin-btn-secondary"
                    type="submit"
                    disabled={frozen() || !matchesBaseline()}
                  >
                    {busy() ? t("admin-action-working") : t("admin-runtime-validate")}
                  </button>
                  <button
                    class="admin-btn-primary"
                    type="button"
                    disabled={!canUpdate()}
                    onClick={onUpdate}
                  >
                    {busy() ? t("admin-action-working") : t("admin-runtime-update")}
                  </button>
                </div>
              </form>
              {/* 写入确认对话框放在表单外，避免 dialog 语义嵌进 <form>。 */}
              <Show when={updateConfirmOpen()}>
                <Show when={command()}>
                  {(pendingCommand) => (
                    <Dialog
                      title={t("admin-runtime-update-confirm-title")}
                      closeLabel={t("admin-users-action-cancel")}
                      busy={busy()}
                      onClose={() => updateConfirmOpen.set(false)}
                    >
                      <p class="admin-panel__note">
                        {t("admin-runtime-update-confirm-summary", {
                          fingerprint: pendingCommand().fingerprint,
                          fields: configChanges().length,
                        })}
                      </p>
                      <ConfigChangeList
                        changes={configChanges()}
                        pendingRestart={restartRequired()}
                      />
                      <div class="admin-shell__dialog-actions">
                        <button
                          class="admin-btn-danger"
                          type="button"
                          disabled={frozen() || !canConfirm()}
                          onClick={onConfirmUpdate}
                        >
                          {busy() ? t("admin-action-working") : t("admin-users-action-confirm")}
                        </button>
                        <button
                          class="admin-btn-secondary"
                          type="button"
                          disabled={busy()}
                          onClick={() => updateConfirmOpen.set(false)}
                        >
                          {t("admin-users-action-cancel")}
                        </button>
                      </div>
                      {/* 常驻播报区域随本轮读取更新，取消后可在页面上重试。 */}
                      <div role="status" aria-atomic="true">
                        {confirmationReadFeedback()}
                      </div>
                    </Dialog>
                  )}
                </Show>
              </Show>
            </div>
          )}
        </Show>
        <ReadFeedbackPanel
          feedback={readFeedback()}
          onRetry={() => advanceCommandReload(refresh)}
        />
        <Show when={reloadConfirmOpen()}>
          <Dialog
            title={t("admin-runtime-discard-title")}
            closeLabel={t("admin-runtime-keep-editing")}
            onClose={() => reloadConfirmOpen.set(false)}
          >
            <p>{t("admin-runtime-discard-detail")}</p>
            <div class="admin-shell__dialog-actions">
              <button
                class="admin-btn-secondary"
                type="button"
                onClick={() => reloadConfirmOpen.set(false)}
              >
                {t("admin-runtime-keep-editing")}
              </button>
              <button
                class="admin-btn-danger"
                type="button"
                disabled={frozen()}
                onClick={discardAndReload}
              >
                {t("admin-runtime-discard-reload")}
              </button>
            </div>
          </Dialog>
        </Show>
      </FormIssuesContext>
    </PendingCommandsContext>
  );
}

function requireConfigDraft(): ConfigDraft {
  const draft = useContext(ConfigDraftContext);
  if (draft === null) {
    throw new Error("配置草稿上下文缺失");
  }
  return draft;
}
