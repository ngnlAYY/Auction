// 凭据页面的数据加载、删除编排和顶层布局。

import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { err } from "@auction/web-shared/result";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, Match, onCleanup, Show, Switch } from "solid-js";

import {
  type AdminCredential,
  type AdminCredentialPage,
  deleteAdminCredential,
  fetchAdminCredentials,
  prepareAdminCredentialDelete,
} from "../../api/credentials";
import { copyCredential } from "../../api/credentials/prepare";
import { useAdminSessionHost } from "../auth/shell";
import { AdminPager, useAdminPagination, useVersionedAdminPage } from "../pagination";
import {
  commandDeliveryReported,
  PendingCommandsContext,
  setIfAlive,
  trySetIfAlive,
} from "../pending";
import { PendingCommandPanel } from "../pending/panel";
import { advanceCommandReload, createCommandReload, usePendingCommands } from "../pending/sources";
import { adminApiErrorMessage } from "../presentation";
import { usePageQueryState } from "../query/credentials";
import type { SortDirection } from "../sort";
import { CredentialDeleteDialog } from "./dialogs";
import { CredentialCreateForm } from "./form";
import { type CredentialFeedback, credentialFeedbackMessage } from "./labels";
import { CredentialTable } from "./table";

type CredentialsView =
  | { readonly kind: "loading" }
  | { readonly kind: "error"; readonly code: string }
  | {
      readonly kind: "ready";
      readonly generation: number;
      readonly page: AdminCredentialPage;
    };

/** 加载并展示 Admin 凭据管理页面。 */
export function page(): JSX.Element {
  const host = useAdminSessionHost();
  const reloadGeneration = createCommandReload(0n);
  const pendingCommands = usePendingCommands(reloadGeneration, (entity) =>
    entity.startsWith("credential:"),
  );
  // 分页跟地址栏双向同步（?page=）；创建表单与删除确认对话框是瞬态，不入 URL。
  const query = usePageQueryState();
  const queryState = query.state;
  const requestedPage = createValueMemo(() => queryState().page);
  const pendingDelete = createCell<AdminCredential | null>(null);
  const deleteFeedback = createCell<CredentialFeedback | null>(null);
  const pagination = useAdminPagination("credentials", ["page_size"], 200, 0x7fffffff);
  const credentials = useVersionedAdminPage(
    pagination,
    () => Number(reloadGeneration()),
    () => requestedPage(),
    (page, size, signal) => {
      if (page > BigInt(Number.MAX_SAFE_INTEGER)) {
        return Promise.resolve(err("admin_pagination_bounds"));
      }
      return fetchAdminCredentials(host, "", "", "", Number(page), size, signal);
    },
  );
  let alive = true;
  onCleanup(() => {
    alive = false;
  });
  const view = createValueMemo((): CredentialsView => {
    const current = credentials();
    if (current === null) {
      return { kind: "loading" };
    }
    if (!current.ok) {
      return { kind: "error", code: current.error };
    }
    const [generation, page] = current.value;
    return { kind: "ready", generation, page };
  });
  const readyView = createValueMemo(() => {
    const current = view();
    return current.kind === "ready" ? current : null;
  });
  const errorView = createValueMemo(() => {
    const current = view();
    return current.kind === "error" ? current : null;
  });
  const pageSize = pagination.sizeSignal();
  const pageCorrection = createValueMemo(() => {
    const current = readyView();
    if (current === null) {
      return null;
    }
    const { generation, page } = current;
    const lastPage = 1 + Math.floor((Math.max(page.total, 1) - 1) / page.page_size);
    return page.page > lastPage
      ? { generation, previousPage: BigInt(page.page), previousSize: page.page_size, lastPage }
      : null;
  });
  // 只有当前快照能纠偏；替换失效页的历史项，避免后退再次进入同一个空页。
  createEffect(pageCorrection, (correction) => {
    const current = queryState.peek();
    if (
      correction !== null &&
      correction.generation === Number(reloadGeneration.peek()) &&
      correction.previousPage === current.page &&
      correction.previousSize === pageSize.peek()
    ) {
      query.replace({ ...current, page: BigInt(correction.lastPage) });
    }
  });
  const credentialEntity = (credential: AdminCredential): string => `credential:${credential.id}`;
  const deleteIsCurrent = (credential: AdminCredential): boolean => {
    const current = credentials();
    // 删除确认冻结了凭据版本；当前页重读或翻页不能把它转成对另一版本的删除。
    if (current === null || !current.ok) {
      return false;
    }
    const [loadedGeneration, page] = current.value;
    return (
      loadedGeneration === Number(reloadGeneration()) &&
      page.credentials.some(
        (candidate) =>
          candidate.id === credential.id && candidate.state_version === credential.state_version,
      )
    );
  };
  const openDelete = (credential: AdminCredential): void => {
    if (deleteIsCurrent(credential) && !pendingCommands.contains(credentialEntity(credential))) {
      deleteFeedback.set(null);
      pendingDelete.set(credential);
    }
  };
  const confirmDelete = (credential: AdminCredential): void => {
    const entity = credentialEntity(credential);
    if (!deleteIsCurrent(credential) || pendingCommands.contains(entity)) {
      return;
    }
    deleteFeedback.set(null);
    // 删除与行内变更共用实体门禁；关闭确认框不能丢弃未知结果的原 UUID 和版本。
    const frozenCredential = copyCredential(credential);
    // 同一命令的手动重试复用首次生成的正文；执行体只读取冻结快照。
    let prepared: ReturnType<typeof prepareAdminCredentialDelete> | null = null;
    if (
      pendingCommands.submitWithReceipt(
        entity,
        t("admin-credentials-action-delete"),
        async (commandId, signal) => {
          if (prepared === null) {
            prepared = prepareAdminCredentialDelete(commandId, frozenCredential);
          }
          return deleteAdminCredential(host, prepared, signal);
        },
        (result, sourceAlive) => {
          const delivered =
            sourceAlive &&
            trySetIfAlive(
              deleteFeedback,
              { kind: "success", restartRequired: result.restart_required },
              () => alive,
            );
          return commandDeliveryReported(delivered, () =>
            result.restart_required
              ? { kind: "text", text: t("admin-credentials-action-restart") }
              : null,
          );
        },
        (code) => setIfAlive(deleteFeedback, { kind: "error", code }, () => alive),
      )
    ) {
      pendingDelete.set(null);
    }
  };

  return (
    <PendingCommandsContext value={pendingCommands}>
      <section class="admin-credentials-page" aria-labelledby="credentials-title">
        <h1 id="credentials-title">{t("page-credentials-title")}</h1>
        <CredentialCreateForm reloadGeneration={reloadGeneration} />
        <PendingCommandPanel owner={pendingCommands} />
        <Switch
          fallback={<AsyncState kind="loading" title={t("admin-credentials-loading")} detail="" />}
        >
          <Match when={errorView()}>
            {(error) => (
              <AsyncState
                kind="error"
                title={adminApiErrorMessage(error().code)}
                detail=""
                retryLabel={t("admin-retry")}
                onRetry={() => advanceCommandReload(reloadGeneration)}
              />
            )}
          </Match>
          <Match when={readyView()}>
            {(ready) => (
              <>
                <CredentialTable
                  page={ready().page}
                  reloadGeneration={reloadGeneration}
                  loadedGeneration={BigInt(ready().generation)}
                  sort={queryState().sort}
                  direction={queryState().direction}
                  onSortChange={(value: readonly [string, SortDirection]) => {
                    const next = { ...queryState(), sort: value[0], direction: value[1] };
                    query.push(next);
                  }}
                  onDelete={openDelete}
                />
                <Show when={pageCorrection() === null}>
                  <AdminPager
                    pagination={pagination}
                    page={ready().page.page}
                    pageSize={ready().page.page_size}
                    total={ready().page.total}
                    label={t("page-credentials-title")}
                    onPageChange={(nextPage) => {
                      const next = { ...queryState(), page: BigInt(nextPage) };
                      query.push(next);
                    }}
                  />
                </Show>
              </>
            )}
          </Match>
        </Switch>
        <Show when={deleteFeedback()}>
          {(feedback) => (
            <p
              class={
                feedback().kind === "success"
                  ? "admin-panel__note action-feedback action-feedback--success"
                  : "admin-panel__note action-feedback action-feedback--error"
              }
              role={feedback().kind === "success" ? "status" : "alert"}
            >
              {credentialFeedbackMessage(feedback())}
            </p>
          )}
        </Show>
      </section>
      <Show when={pendingDelete()}>
        {(credential) => (
          <CredentialDeleteDialog
            busy={pendingCommands.isBusy(credentialEntity(credential()))}
            blocked={
              !deleteIsCurrent(credential()) ||
              pendingCommands.contains(credentialEntity(credential()))
            }
            credential={credential()}
            onConfirm={confirmDelete}
            onCancel={() => pendingDelete.set(null)}
          />
        )}
      </Show>
    </PendingCommandsContext>
  );
}
