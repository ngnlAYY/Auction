// 场次与拍品编辑共用的列表读取、冲突反馈与字段聚焦。

import { focusElementById } from "@auction/web-shared/platform/a11y";
import { sleep } from "@auction/web-shared/platform/timer";
import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { structuralEquals } from "@auction/web-shared/reactive/equality";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { ok, type Result } from "@auction/web-shared/result";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { type Accessor, Match, Show, Switch } from "solid-js";

import {
  type AdminWriteFailure,
  adminWriteFailureDisplayCode,
  classifyAdminWriteFailure,
} from "../../api/failure";
import { readVerifiedWriteReceipt } from "../../api/transport";
import { type AdminPagination, useAdminPage } from "../pagination";
import { setIfAlive } from "../pending";
import { advanceCommandReload } from "../pending/sources";
import { adminApiErrorMessage } from "../presentation";

/** 最新一轮列表读取的状态。保留的旧快照只用来展示。 */
export type DraftRead =
  | { readonly kind: "loading" }
  | { readonly kind: "ready" }
  | { readonly kind: "failed"; readonly code: string };

/** 只允许从成功完成的当前读取打开草稿。 */
export function draftReadIsReady(read: DraftRead): boolean {
  return read.kind === "ready";
}

/** 冲突触发的读取已经在途或完成时，关闭只丢弃旧草稿。 */
export function draftReadRetryAfterClose(read: DraftRead, conflict: boolean): boolean {
  return conflict && read.kind === "failed";
}

/** 冻结的版本只用于核对；列表换页而未找到实体不能证明其版本变化。 */
export function draftReadVersionChanged(
  read: DraftRead,
  opened: number,
  latest: number | null,
): boolean {
  return read.kind === "ready" && latest !== null && latest !== opened;
}

interface DraftSnapshot<Q, T> {
  readonly identity: Q;
  readonly size: number;
  readonly page: T;
}

/**
 * 草稿由页面单独持有。这里仅在同一查询下保留成功快照，供刷新期间或编辑中的读取失败展示。
 */
export function useDraftPage<Q, T>(
  pagination: AdminPagination,
  reload: Cell<bigint>,
  editing: Accessor<boolean>,
  request: () => Q,
  fetch: (query: Q, size: number, signal: AbortSignal) => Promise<Result<T, string>>,
): readonly [Accessor<DraftRead>, Accessor<Result<T, string> | null>] {
  const query = createValueMemo(request);
  const pageSize = pagination.sizeSignal();
  const snapshot = createCell<DraftSnapshot<Q, T> | null>(null);
  const result = useAdminPage(
    pagination,
    () => [query(), reload()] as const,
    async ([identity, generation], size, signal) => {
      const page = await fetch(identity, size, signal);
      if (
        page.ok &&
        !signal.aborted &&
        structuralEquals(query(), identity) &&
        pageSize.peek() === size &&
        reload.peek() === generation
      ) {
        // 旧查询或旧代次完成时不能覆盖当前页的保留快照。
        snapshot.set({ identity, size, page: page.value });
      }
      return page;
    },
  );

  const read = createValueMemo((): DraftRead => {
    const current = result();
    if (current === null) {
      return { kind: "loading" };
    }
    return current.ok ? { kind: "ready" } : { kind: "failed", code: current.error };
  });
  const pageResult = createValueMemo((): Result<T, string> | null => {
    const current = result();
    if (current === null || (editing() && !current.ok)) {
      const cached = snapshot();
      if (
        cached !== null &&
        structuralEquals(cached.identity, query()) &&
        cached.size === pageSize()
      ) {
        return ok(cached.page);
      }
    }
    return current;
  });
  return [read, pageResult];
}

/** 对话框内分别反馈版本冲突、写入错误与读取状态。 */
export function DraftFeedback(props: {
  readonly conflict: boolean;
  readonly read: DraftRead;
  readonly failure: string | null;
}): JSX.Element {
  const failureMessage = createValueMemo(() =>
    props.failure === null ? null : adminApiErrorMessage(props.failure),
  );
  const failedCode = createValueMemo(() => (props.read.kind === "failed" ? props.read.code : null));
  return (
    <Show
      when={props.conflict}
      fallback={
        <>
          <Show when={failureMessage() !== null}>
            <p role="alert" class="action-feedback action-feedback--error">
              {failureMessage() ?? ""}
            </p>
          </Show>
          <Switch>
            <Match when={props.read.kind === "loading"}>
              <p role="status">{t("admin-record-refresh-loading")}</p>
            </Match>
            <Match when={failedCode() !== null}>
              <p role="alert" class="action-feedback action-feedback--error">
                {t("admin-record-refresh-failed", {
                  detail: adminApiErrorMessage(failedCode() ?? ""),
                })}
              </p>
            </Match>
          </Switch>
        </>
      }
    >
      <p role="alert" class="action-feedback action-feedback--error">
        {t("admin-record-stale")}
      </p>
    </Show>
  );
}

/** 普通读取保留草稿；明确发生版本冲突时只能关闭草稿，再从刷新的行创建新意图。 */
export function DraftRefreshButton(props: {
  readonly read: DraftRead;
  readonly conflict: boolean;
  readonly reload: Cell<bigint>;
  readonly onClose: () => void;
}): JSX.Element {
  const loading = () => !props.conflict && props.read.kind === "loading";
  const label = createValueMemo(() => {
    if (draftReadRetryAfterClose(props.read, props.conflict)) {
      return t("admin-record-close-and-refresh");
    }
    if (props.conflict) {
      return t("admin-record-close");
    }
    if (props.read.kind === "failed") {
      return t("admin-retry");
    }
    return t("admin-record-refresh");
  });
  return (
    <button
      type="button"
      class={props.conflict ? "admin-btn-primary" : ""}
      disabled={loading()}
      onClick={() => {
        if (loading()) {
          return;
        }
        if (props.conflict) {
          props.onClose();
        } else {
          advanceCommandReload(props.reload);
        }
      }}
    >
      {label()}
    </button>
  );
}

/** 冲突后的权威读取失败才在关闭时重试，未完成的读取继续约束新草稿。 */
export function closeEditDraft<T>(
  draft: Cell<T | null>,
  reload: Cell<bigint>,
  retry: boolean,
): void {
  draft.set(null);
  if (retry) {
    advanceCommandReload(reload);
  }
}

/** 草稿的显示码与原围栏是否已被实际写回执拒绝。 */
export interface DraftWriteFeedback {
  readonly displayCode: string;
  readonly staleWrite: boolean;
}

/** 只有已验证的版本拒绝会锁定草稿，前置读取的同名错误只用于展示。 */
export async function observeDraftWrite<T>(
  request: Promise<Result<T, AdminWriteFailure>>,
  staleCode: "stale_auction_state" | "stale_lot_state",
  feedback: Cell<DraftWriteFeedback | null>,
  alive: () => boolean,
): Promise<Result<T, AdminWriteFailure>> {
  const result = await request;
  const outcome = result.ok
    ? null
    : {
        displayCode: adminWriteFailureDisplayCode(result.error),
        staleWrite:
          result.error.kind === "verified-http" &&
          readVerifiedWriteReceipt(result.error.receipt)?.protocolCode === staleCode &&
          classifyAdminWriteFailure(result.error) === "replay_checked",
      };
  setIfAlive(feedback, outcome, alive);
  return result;
}

/** 从未决区完成的旧命令只收起自己的草稿。 */
export function closeMatchingDraft<T>(
  draft: Cell<T | null>,
  matches: (current: T) => boolean,
): void {
  const current = draft.peek();
  if (current !== null && matches(current)) {
    draft.set(null);
  }
}

/** 动态字段 ID 也须等错误节点及 ARIA 属性写入 DOM 后才聚焦。 */
export function focusDraftFieldAfterRender(fieldId: string): void {
  const scope = focusScopeOf(fieldId);
  void sleep(0).then((completed) => {
    if (!completed) {
      return;
    }
    // 草稿对话框卸载后，旧任务不能聚焦后来挂载的同 ID 控件。
    if (scope !== null) {
      const target = document.getElementById(fieldId);
      if (!scope.isConnected || target === null || !scope.contains(target)) {
        return;
      }
    }
    focusElementById(fieldId);
  });
}

/** 字段所在的模态对话框；字段尚未写入 DOM 时按当前焦点所在的对话框兜底。 */
function focusScopeOf(fieldId: string): HTMLElement | null {
  if (typeof document === "undefined") {
    return null;
  }
  const field = document.getElementById(fieldId);
  const fromField = field?.closest('[role="dialog"][aria-modal="true"]');
  if (fromField instanceof HTMLElement) {
    return fromField;
  }
  const active = document.activeElement;
  const fromActive =
    active instanceof Element ? active.closest('[role="dialog"][aria-modal="true"]') : null;
  return fromActive instanceof HTMLElement ? fromActive : null;
}
