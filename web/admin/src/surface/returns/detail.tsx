// 结算退件详情页：摘要与资产位置说明、处理动作与阻断原因、领回（Take）、代管、派发链与事件。
//
// 字段全部来自服务端安全投影；按钮只按服务端给出的 `allowed_actions` 展示，命令事务仍会锁内
// 复核。页面明确区分“已寄出”（邮件发出）、“退回仍在收件箱”、“已领回”与“代管”。

import type { AdminSettlementReturnDetailResponse } from "@auction/contracts/dto/admin/returns";
import { focusElementById } from "@auction/web-shared/platform/a11y";
import { topModal } from "@auction/web-shared/platform/a11y/escape";
import { sleep } from "@auction/web-shared/platform/timer";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import type { Result } from "@auction/web-shared/result";
import { Link } from "@auction/web-shared/router";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import { validUuid } from "@auction/web-shared/ui/validation";
import type { JSX } from "@solidjs/web";
import { For, Match, Show, Switch, untrack } from "solid-js";

import { fetchSettlementReturn } from "../../api/returns";
import { useAdminSessionHost } from "../auth/shell";
import { PendingCommandsContext } from "../pending";
import { PendingCommandPanel } from "../pending/panel";
import { advanceCommandReload, createCommandReload, usePendingCommands } from "../pending/sources";
import { type AdminActionFeedback, adminApiErrorMessage } from "../presentation";
import { ActionSection } from "./actions";
import {
  CustodySection,
  DispatchSection,
  EventSection,
  SummarySection,
  TakeSection,
} from "./detail-sections";
import { showsAssetLocation } from "./labels";

/** 路由复用同一组件实例，按案件身份重挂载资源与一次性状态。 */
export function detailPage(returnId: string): JSX.Element {
  return (
    <For each={[returnId]} keyed={(value) => value}>
      {(value) => {
        const id = untrack(() => value());
        return <ReturnDetailPage returnId={id} />;
      }}
    </For>
  );
}

interface ReturnDetailPageProps {
  readonly returnId: string;
}

interface DetailRead {
  readonly returnId: string;
  readonly generation: bigint;
  readonly result: Result<AdminSettlementReturnDetailResponse, string>;
}

interface ErrorCode {
  readonly code: string;
}

/** 单条退件案件的详情与处理入口。 */
function ReturnDetailPage(props: ReturnDetailPageProps): JSX.Element {
  const host = useAdminSessionHost();
  const returnId = untrack(() => props.returnId);
  const refresh = createCommandReload(0n);
  // 命令成功后宿主递增 refresh 重读详情；结果未知的命令留在未决区按原 command_id 重试。
  const target = `settlement-return:${returnId}`;
  const commands = usePendingCommands(refresh, (entity) => entity === target);
  const notice = createCell<AdminActionFeedback | null>(null);
  // 动作区会随详情重读而卸载，还焦任务由保留标题的详情页持有。
  const focusTasks = createTaskScope();
  const restoreFocus = (): void => {
    focusTasks.spawn(async (context) => {
      if (
        !(await sleep(0, context.signal)) ||
        !context.alive() ||
        typeof document === "undefined"
      ) {
        return;
      }
      const focus = document.activeElement;
      if (
        (focus === null || focus === document.body || focus === document.documentElement) &&
        topModal(document) === null
      ) {
        focusElementById("settlement-return-detail-title");
      }
    });
  };
  const detail = createTaskResource(
    () => [returnId, refresh()] as const,
    async ([id, generation], context): Promise<DetailRead> => {
      const result = await fetchSettlementReturn(host, id, context.signal);
      return { returnId: id, generation, result };
    },
  );
  // 刷新期间不展示上一轮结果，避免按旧版本提交命令。
  const loaded = createValueMemo((): Result<AdminSettlementReturnDetailResponse, string> | null => {
    if (detail.state() !== "ready") {
      return null;
    }
    const snapshot = detail();
    if (
      snapshot === undefined ||
      snapshot.returnId !== returnId ||
      snapshot.generation !== refresh()
    ) {
      return null;
    }
    return snapshot.result;
  });
  const canRefresh = createValueMemo(() => loaded()?.ok === true);
  // 非法标识不会产生可重试请求：无论读取是否完成都直接展示标识错误，不提供无意义的刷新。
  const invalidIdView = createValueMemo(() => !validUuid(returnId));
  const loadingView = createValueMemo(() => loaded() === null);
  const errorView = createValueMemo((): ErrorCode | null => {
    const current = loaded();
    return current !== null && !current.ok ? { code: current.error } : null;
  });
  const loadedProjection = createValueMemo((): AdminSettlementReturnDetailResponse | null => {
    const current = loaded();
    return current?.ok ? current.value : null;
  });
  const noticeError = createValueMemo((): ErrorCode | null => {
    const current = notice();
    return current !== null && current.kind === "error" ? { code: current.code } : null;
  });

  return (
    <PendingCommandsContext value={commands}>
      <section
        class="admin-settlement-return-detail"
        aria-labelledby="settlement-return-detail-title"
      >
        <div class="admin-settlement-return-detail__head">
          <h1 id="settlement-return-detail-title" tabindex="-1">
            {t("settlement-return-detail-title")}
          </h1>
          <Link href="/settlement-returns" class="admin-settlement-return-detail__back">
            {t("settlement-return-detail-back")}
          </Link>
          <Show when={canRefresh()}>
            <button type="button" onClick={() => advanceCommandReload(refresh)}>
              {t("settlement-return-refresh-case")}
            </button>
          </Show>
        </div>
        <PendingCommandPanel owner={commands} />
        {/* 案件详情重读会卸载动作区，处理回执仍须保留。 */}
        <div role="status">
          <Show when={notice()?.kind === "success"}>
            <p class="action-feedback action-feedback--success">
              {t("settlement-return-action-success")}
            </p>
          </Show>
        </div>
        <Show when={noticeError()}>
          {(error) => (
            <p class="action-feedback action-feedback--error" role="alert">
              {adminApiErrorMessage(error().code)}
            </p>
          )}
        </Show>
        <Switch fallback={null}>
          <Match when={invalidIdView()}>
            <AsyncState
              kind="error"
              title={adminApiErrorMessage("invalid_settlement_return_id")}
              detail=""
            />
          </Match>
          <Match when={loadingView()}>
            <AsyncState kind="loading" title={t("settlement-return-detail-loading")} detail="" />
          </Match>
          <Match when={errorView()}>
            {(error) => (
              <AsyncState
                kind="error"
                title={adminApiErrorMessage(error().code)}
                detail=""
                retryLabel={t("admin-retry")}
                onRetry={() => advanceCommandReload(refresh)}
              />
            )}
          </Match>
          <Match when={loadedProjection()}>
            {(loadedDetail) => (
              <>
                {/* 待重新核验但没有能读取该世界 MoogleMail 的在线 Worker：说明案件为何不前进。 */}
                <Show when={loadedDetail().recheck_unavailable}>
                  <p class="action-feedback action-feedback--error" role="note">
                    {t("settlement-return-recheck-unavailable")}
                  </p>
                </Show>
                {/* 待领回但没有 Worker 能以数据库凭据身份执行该世界的 Take：说明案件为何不前进。 */}
                <Show when={loadedDetail().take_unavailable}>
                  <p class="action-feedback action-feedback--error" role="note">
                    {t("settlement-return-take-unavailable")}
                  </p>
                </Show>
                <SummarySection summary={loadedDetail().summary} />
                <ActionSection
                  detail={loadedDetail()}
                  notice={notice}
                  refresh={refresh}
                  loadedGeneration={refresh()}
                  restoreFocus={restoreFocus}
                />
                <Show when={showsAssetLocation(loadedDetail().summary.status)}>
                  <TakeSection
                    take={loadedDetail().take}
                    status={loadedDetail().summary.status}
                    unknownReason={loadedDetail().take_unknown_reason}
                  />
                  <Show when={loadedDetail().custody}>
                    {(custody) => <CustodySection custody={custody()} />}
                  </Show>
                </Show>
                <DispatchSection
                  dispatches={loadedDetail().dispatches}
                  returnedDelivery={loadedDetail().summary.delivery_id}
                  resendDispatchNo={loadedDetail().resend_dispatch_no}
                />
                <EventSection events={loadedDetail().events} />
              </>
            )}
          </Match>
        </Switch>
      </section>
    </PendingCommandsContext>
  );
}
