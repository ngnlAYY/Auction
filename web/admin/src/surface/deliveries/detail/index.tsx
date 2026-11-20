// Admin 投递详情页：冻结尝试、可引用 Sent 证据、领取状态、处置与来源义务。
//
// 只读聚合与人工 Sent 证明入口；所有字段都来自 `auction-contracts` 的安全投影，
// 页面不展示投递 payload、正文、Cookie 或登录 key，也不在浏览器重建状态机。

import {
  type AdminDeliveryDetailWithActionsResponse,
  adminDeliveryActionAvailabilityBlocker,
} from "@auction/contracts/dto/admin/delivery";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { err, ok, type Result } from "@auction/web-shared/result";
import { Link } from "@auction/web-shared/router";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import { validUuid } from "@auction/web-shared/ui/validation";
import type { JSX } from "@solidjs/web";
import { For, onCleanup, Show, untrack } from "solid-js";

import {
  acknowledgeDelivery,
  confirmDeliverySentProof,
  fetchDeliveryDetail,
  prepareAcknowledgeDelivery,
  prepareSentProofCommand,
  type SentProofCommand,
} from "../../../api/delivery";
import { trimRustWhitespace } from "../../../api/session-time";
import { useAdminSessionHost } from "../../auth/shell";
import { PendingCommandsContext, setIfAlive } from "../../pending";
import { PendingCommandPanel } from "../../pending/panel";
import {
  advanceCommandReload,
  createCommandReload,
  usePendingCommands,
} from "../../pending/sources";
import { adminApiErrorMessage } from "../../presentation";
import { deliveryCommandEntity } from "..";
import { actionBlockerLabel } from "../labels";
import { selectableEvidence } from "../sent-proof/evidence";
import { SentProofDialog } from "../sent-proof/render";
import type { SentProofSubmission } from "../sent-proof/state";
import {
  AttemptSection,
  CustodySection,
  DeliverySummary,
  DispositionSection,
  EvidenceSection,
  ObligationSection,
} from "./sections";

interface DetailRead {
  readonly deliveryId: string;
  readonly revision: bigint;
  readonly result: Result<AdminDeliveryDetailWithActionsResponse, string>;
}

/** 路由复用的是同一组件实例，因此按投递身份重挂载资源与一次性状态。 */
export function detailPage(deliveryId: string): JSX.Element {
  return (
    <For each={[deliveryId]} keyed={(value) => value}>
      {(value) => {
        const id = untrack(() => value());
        return <DeliveryDetailPage deliveryId={id} />;
      }}
    </For>
  );
}

interface DeliveryDetailPageProps {
  readonly deliveryId: string;
}

/** 单条投递的详情与人工处置入口。 */
function DeliveryDetailPage(props: DeliveryDetailPageProps): JSX.Element {
  const host = useAdminSessionHost();
  const deliveryId = untrack(() => props.deliveryId);
  const entity = deliveryCommandEntity(deliveryId);
  const refresh = createCommandReload(0n);
  const commands = usePendingCommands(refresh, (candidate) => candidate === entity);
  const occupied = (): boolean => commands.contains(entity);
  const blockedNote = (): string | null => (occupied() ? "admin-delivery-command-occupied" : null);
  const busy = (): boolean => commands.anyBusy();
  const actionError = createCell<string | null>(null);
  const sentOpen = createCell(false);
  const sentSubmitted = createCell(false);
  const acknowledgeReason = createCell("");
  const acknowledged = createCell(false);
  let alive = true;
  onCleanup(() => {
    alive = false;
  });

  const detail = createTaskResource(
    () => [deliveryId, refresh()] as const,
    async ([id, revision], context): Promise<DetailRead> => {
      const result = await fetchDeliveryDetail(host, id, context.signal);
      return { deliveryId: id, revision, result };
    },
  );
  // 刷新信号变化即使旧详情失效，避免资源尚未重启时重新打开旧证据。
  const loaded = createValueMemo(
    (): Result<AdminDeliveryDetailWithActionsResponse, string> | null => {
      if (detail.state() !== "ready") {
        return null;
      }
      const snapshot = detail();
      if (
        snapshot === undefined ||
        snapshot.deliveryId !== deliveryId ||
        snapshot.revision !== refresh()
      ) {
        return null;
      }
      return snapshot.result;
    },
  );
  const invalidIdView = createValueMemo(() => loaded() === null && !validUuid(deliveryId));
  const loadingView = createValueMemo(() => loaded() === null && validUuid(deliveryId));
  const errorView = createValueMemo((): string | null => {
    const result = loaded();
    return result !== null && !result.ok ? result.error : null;
  });
  const loadedProjection = createValueMemo((): AdminDeliveryDetailWithActionsResponse | null => {
    const result = loaded();
    return result?.ok ? result.value : null;
  });

  const confirmSentProof = (submission: SentProofSubmission): void => {
    if (busy() || occupied()) {
      return;
    }
    actionError.set(null);
    sentSubmitted.set(false);
    // 提交时冻结命令参数；执行体与重试都不再读取对话框里的草稿。
    const command: SentProofCommand = {
      deliveryId,
      expectedFencingVersion: submission.expectedFencingVersion,
      reason: submission.reason,
      evidenceReference: submission.evidenceReference,
      evidence: submission.evidence,
      acceptCollectedAssets: submission.acceptCollectedAssets,
      acceptAttemptHistory: submission.acceptAttemptHistory,
    };
    const prepare = prepareSentProofCommand(command);
    const submitted = commands.submit(
      entity,
      t("delivery-sent-proof-confirm"),
      async (commandId, signal) => {
        const prepared = prepare(commandId);
        if (!prepared.ok) {
          return err(prepared.error.code);
        }
        const result = await confirmDeliverySentProof(host, prepared.value, signal);
        return result.ok ? ok(result.value) : err(result.error.code);
      },
      () => {
        setIfAlive(sentOpen, false, () => alive);
        setIfAlive(sentSubmitted, true, () => alive);
      },
      (code) => {
        if (code === "stale_delivery_state" || code === "stale_state_version") {
          advanceCommandReload(refresh);
        }
        setIfAlive(actionError, code, () => alive);
      },
    );
    if (submitted) {
      sentOpen.set(false);
    }
  };

  const acknowledgeFencingVersion = (): number | null => {
    const current = loadedProjection();
    if (
      current === null ||
      adminDeliveryActionAvailabilityBlocker(current.actions.acknowledge) !== null
    ) {
      return null;
    }
    return current.detail.delivery.fencing_version;
  };

  const acknowledge = (): void => {
    const fencingVersion = acknowledgeFencingVersion();
    if (fencingVersion === null) {
      return;
    }
    const reason = trimRustWhitespace(acknowledgeReason());
    if (reason === "" || busy() || occupied()) {
      return;
    }
    actionError.set(null);
    const prepare = prepareAcknowledgeDelivery(deliveryId, fencingVersion, reason);
    commands.submit(
      entity,
      t("delivery-detail-acknowledge-submit"),
      async (commandId, signal) => {
        const prepared = prepare(commandId);
        if (!prepared.ok) {
          return err(prepared.error.code);
        }
        const result = await acknowledgeDelivery(host, prepared.value, signal);
        return result.ok ? ok(null) : err(result.error.code);
      },
      () => {
        setIfAlive(acknowledged, true, () => alive);
        setIfAlive(acknowledgeReason, "", () => alive);
        advanceCommandReload(refresh);
      },
      (code) => {
        if (code === "stale_delivery_state" || code === "stale_state_version") {
          advanceCommandReload(refresh);
        }
        setIfAlive(actionError, code, () => alive);
      },
    );
  };

  return (
    <PendingCommandsContext value={commands}>
      <section class="admin-delivery-detail" aria-labelledby="delivery-detail-title">
        <div class="admin-delivery-detail__head">
          <h1 id="delivery-detail-title">{t("delivery-detail-title")}</h1>
          <Link href="/deliveries" class="admin-delivery-detail__back">
            {t("delivery-detail-back")}
          </Link>
        </div>
        <PendingCommandPanel owner={commands} />
        <Show when={blockedNote()}>
          {(note) => (
            <p id={note()} class="admin-panel__note">
              {t("delivery-command-occupied")}
            </p>
          )}
        </Show>
        <Show
          when={invalidIdView()}
          fallback={
            <Show
              when={loadingView()}
              fallback={
                <Show
                  when={errorView()}
                  fallback={
                    <Show when={loadedProjection()}>
                      {(projection) => (
                        <>
                          <DeliverySummary detail={projection().detail} />
                          <AttemptSection attempts={projection().detail.attempts} />
                          <EvidenceSection evidence={projection().detail.evidence} />
                          <CustodySection custody={projection().detail.custody} />
                          <DispositionSection dispositions={projection().detail.dispositions} />
                          <ObligationSection
                            obligation={projection().detail.obligation}
                            recovery={projection().detail.recovery}
                          />
                        </>
                      )}
                    </Show>
                  }
                >
                  {(code) => (
                    <AsyncState
                      kind="error"
                      title={adminApiErrorMessage(code())}
                      detail=""
                      retryLabel={t("admin-retry")}
                      onRetry={() => advanceCommandReload(refresh)}
                    />
                  )}
                </Show>
              }
            >
              <AsyncState kind="loading" title={t("delivery-detail-loading")} detail="" />
            </Show>
          }
        >
          <AsyncState kind="error" title={adminApiErrorMessage("invalid_delivery_id")} detail="" />
        </Show>
        {/* 详情与证明入口随读取卸载，已接受的写入仍由投递页面交付。 */}
        <div role="status">
          <Show when={acknowledged()}>
            <p class="action-feedback action-feedback--success">
              {t("delivery-detail-acknowledge-success")}
            </p>
          </Show>
          <Show when={sentSubmitted()}>
            <p class="action-feedback action-feedback--success">
              {t("delivery-sent-proof-submitted")}
            </p>
          </Show>
        </div>
        <Show when={actionError()}>
          {(code) => (
            <p class="action-feedback action-feedback--error" role="alert">
              {adminApiErrorMessage(code())}
            </p>
          )}
        </Show>
        <Show when={loadedProjection()}>
          {(projection) => {
            const blocker = createValueMemo(() =>
              adminDeliveryActionAvailabilityBlocker(projection().actions.acknowledge),
            );
            return (
              <Show when={blocker() !== "status_mismatch"} fallback={null}>
                <Show
                  when={blocker()}
                  fallback={
                    <section
                      class="admin-panel admin-delivery-acknowledge"
                      aria-labelledby="delivery-acknowledge-title"
                    >
                      <h2 id="delivery-acknowledge-title">
                        {t("delivery-detail-acknowledge-title")}
                      </h2>
                      <p class="admin-panel__note">{t("delivery-detail-acknowledge-intro")}</p>
                      <label class="admin-field">
                        <span>{t("delivery-detail-acknowledge-reason")}</span>
                        <textarea
                          rows="3"
                          maxlength="2048"
                          disabled={occupied() || busy()}
                          aria-describedby={blockedNote() ?? undefined}
                          value={acknowledgeReason()}
                          onInput={(event) => acknowledgeReason.set(event.currentTarget.value)}
                        />
                      </label>
                      <button
                        type="button"
                        class="admin-btn-primary"
                        disabled={
                          occupied() || busy() || trimRustWhitespace(acknowledgeReason()) === ""
                        }
                        aria-describedby={blockedNote() ?? undefined}
                        onClick={acknowledge}
                      >
                        {t("delivery-detail-acknowledge-submit")}
                      </button>
                    </section>
                  }
                >
                  {(current) => <p class="admin-panel__note">{actionBlockerLabel(current())}</p>}
                </Show>
              </Show>
            );
          }}
        </Show>
        <Show when={loadedProjection()}>
          {(projection) => {
            const candidates = createValueMemo(
              () => selectableEvidence(projection().detail).length > 0,
            );
            const canProve = createValueMemo(() => {
              const status = projection().detail.delivery.status;
              return (status === "manual_reconcile" || status === "failed") && candidates();
            });
            return (
              <>
                {/* 证明入口只有在存在可引用证据且投递处于可准入状态时出现；服务端仍会重新判定。 */}
                <section
                  class="admin-panel admin-delivery-sent-proof-entry"
                  aria-labelledby="delivery-sent-proof-entry-title"
                >
                  <h2 id="delivery-sent-proof-entry-title">{t("delivery-detail-sent-proof")}</h2>
                  <p class="admin-panel__note">{t("delivery-detail-sent-proof-intro")}</p>
                  <Show
                    when={canProve()}
                    fallback={
                      <Show
                        when={!candidates()}
                        fallback={
                          <p class="admin-panel__note">
                            {t("delivery-detail-sent-proof-not-applicable")}
                          </p>
                        }
                      >
                        <p class="admin-panel__note">
                          {t("delivery-detail-sent-proof-no-evidence")}
                        </p>
                      </Show>
                    }
                  >
                    <button
                      type="button"
                      class="admin-btn-primary"
                      disabled={occupied() || busy()}
                      aria-describedby={blockedNote() ?? undefined}
                      onClick={() => {
                        if (occupied() || busy()) {
                          return;
                        }
                        actionError.set(null);
                        sentOpen.set(true);
                      }}
                    >
                      {t("delivery-detail-sent-proof-open")}
                    </button>
                  </Show>
                </section>
                <Show when={sentOpen()}>
                  <SentProofDialog
                    detail={projection().detail}
                    busy={busy()}
                    blocked={occupied()}
                    onSubmit={confirmSentProof}
                    onCancel={() => sentOpen.set(false)}
                    onStale={() => {
                      // 预览已核对请求序号。关闭旧证据的对话框后，重开的入口只使用新详情。
                      sentOpen.set(false);
                      actionError.set("stale_delivery_state");
                      advanceCommandReload(refresh);
                    }}
                  />
                </Show>
              </>
            );
          }}
        </Show>
      </section>
    </PendingCommandsContext>
  );
}
