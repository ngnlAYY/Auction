// 站点文档（规则、关于）页面加载、语言切换和 workspace 装配。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import type { Result } from "@auction/web-shared/result";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, Match, Show, Switch, useContext } from "solid-js";

import {
  type AdminDocumentKind,
  type AdminRules,
  type AdminRulesApiError,
  adminDocumentKindPendingKey,
  fetchAdminRules,
} from "../../api/rules";
import { useAdminSessionHost } from "../auth/shell";
import { PendingCommandsContext } from "../pending";
import { PendingCommandPanel } from "../pending/panel";
import { advanceCommandReload, usePendingCommands } from "../pending/sources";
import { adminApiErrorMessage } from "../presentation";
import { missingRulesDocument, noticeText, toDraft } from "./draft";
import {
  DocumentDraftsContext,
  documentDraftsGet,
  rulesBaselineFromDocument,
  rulesBaselineMatchesCandidate,
  rulesBaselineMatchesDocument,
  rulesBaselineMatchesLocale,
} from "./model";
import { AdminRulesWorkspace } from "./workspace";

interface DocumentRead {
  readonly locale: string;
  readonly generation: bigint;
  readonly result: Result<AdminRules, AdminRulesApiError>;
}

type DocumentView =
  | { readonly kind: "loading" }
  | { readonly kind: "missing"; readonly document: AdminRules }
  | { readonly kind: "error"; readonly code: string }
  | { readonly kind: "ready"; readonly document: AdminRules };

/** 读取并展示一种 Admin 站点文档；语言切换会让请求带上旧版约定的 `lang` 查询参数。 */
export function page(kind: AdminDocumentKind): JSX.Element {
  const drafts = useContext(DocumentDraftsContext);
  if (drafts === null) {
    throw new Error("站点文档草稿上下文缺失");
  }
  const { target, draft, baseline, checkedCandidate, notice, busy, reloadGeneration, dirty } =
    documentDraftsGet(drafts, kind);
  const host = useAdminSessionHost();
  const pending = usePendingCommands(
    reloadGeneration,
    (entity) => entity === adminDocumentKindPendingKey(kind),
  );
  const rules = createTaskResource(
    () => [reloadGeneration(), target()] as const,
    async ([generation, locale], context): Promise<DocumentRead> => {
      const result = await fetchAdminRules(host, kind, locale, context.signal);
      // 依赖变化会由资源取消旧任务；结果仍携带请求时的语言与世代，供页面二次过滤。
      return { locale, generation, result };
    },
  );

  // 回页时先核对权威语言与版本；草稿有修改时绝不被读取结果覆盖。
  createEffect(
    () => [target(), reloadGeneration(), rules(), rules.state()] as const,
    ([requestedLocale, requestedGeneration, snapshot, state]) => {
      if (state !== "ready" || snapshot === undefined) {
        return;
      }
      if (snapshot.locale !== requestedLocale || snapshot.generation !== requestedGeneration) {
        return;
      }
      const result = snapshot.result;
      let document: AdminRules;
      if (result.ok) {
        if (result.value.locale !== requestedLocale) {
          return;
        }
        document = result.value;
      } else if (result.error.status === 404) {
        document = missingRulesDocument(requestedLocale);
      } else {
        return;
      }
      const base = baseline.peek();
      const candidate = draft.peek();
      const hasEdits =
        base !== null &&
        rulesBaselineMatchesLocale(base, requestedLocale) &&
        candidate !== null &&
        !rulesBaselineMatchesCandidate(base, candidate);
      if (hasEdits) {
        if (base === null || !rulesBaselineMatchesDocument(base, document)) {
          checkedCandidate.set(null);
          notice.set(null);
        }
        return;
      }
      baseline.set(rulesBaselineFromDocument(document));
      draft.set(toDraft(document, requestedLocale));
      checkedCandidate.set(null);
      // 成功反馈属于已完成的发布，不因后续权威快照落地而消失。
      if (notice.peek()?.kind !== "published") {
        notice.set(null);
      }
    },
  );

  // Resource 重读时保留旧值；旧语言或旧世代的响应均不能装配编辑器。
  const current = createValueMemo((): Result<AdminRules, AdminRulesApiError> | null => {
    if (rules.state() !== "ready") {
      return null;
    }
    const snapshot = rules();
    if (
      snapshot === undefined ||
      snapshot.locale !== target() ||
      snapshot.generation !== reloadGeneration()
    ) {
      return null;
    }
    return snapshot.result;
  });
  const baselineReady = createValueMemo((): boolean => {
    const base = baseline();
    return base !== null && rulesBaselineMatchesLocale(base, target());
  });
  const view = createValueMemo((): DocumentView => {
    const result = current();
    if (result === null) {
      return { kind: "loading" };
    }
    const needsBaseline = result.ok || result.error.status === 404;
    if (needsBaseline && !baselineReady()) {
      return { kind: "loading" };
    }
    if (result.ok) {
      return { kind: "ready", document: result.value };
    }
    if (result.error.status === 404) {
      return { kind: "missing", document: missingRulesDocument(target()) };
    }
    return { kind: "error", code: result.error.code };
  });
  const missingView = createValueMemo((): { readonly document: AdminRules } | null => {
    const currentView = view();
    return currentView.kind === "missing" ? currentView : null;
  });
  const errorView = createValueMemo((): { readonly code: string } | null => {
    const currentView = view();
    return currentView.kind === "error" ? currentView : null;
  });
  const readyDocument = createValueMemo((): AdminRules | null => {
    const currentView = view();
    return currentView.kind === "ready" ? currentView.document : null;
  });

  return (
    <PendingCommandsContext value={pending}>
      <section class="admin-rules-page" aria-labelledby="rules-title">
        <h1 id="rules-title">{kind === "rules" ? t("page-rules-title") : t("page-about-title")}</h1>
        <p class="admin-rules-intro">
          {kind === "rules" ? t("admin-rules-description") : t("admin-about-description")}
        </p>
        <PendingCommandPanel owner={pending} />
        {/* 发布清理草稿后编辑器会卸载，已接受的发布仍由文档页面播报。 */}
        <div role="status">
          <Show when={notice()?.kind === "published"}>
            <p class="admin-rules-ok">{noticeText({ kind: "published" })}</p>
          </Show>
        </div>
        <Switch fallback={<AsyncState kind="loading" title={t("admin-rules-loading")} detail="" />}>
          <Match when={missingView()}>
            {(missing) => (
              <>
                <p class="admin-rules-status" role="status">
                  {t("admin-rules-missing")}
                </p>
                <AdminRulesWorkspace
                  kind={kind}
                  document={missing().document}
                  target={target()}
                  targetSignal={target}
                  draft={draft}
                  checkedCandidate={checkedCandidate}
                  baseline={baseline}
                  notice={notice}
                  busy={busy}
                  reloadGeneration={reloadGeneration}
                  pending={pending}
                  dirty={dirty}
                />
              </>
            )}
          </Match>
          <Match when={errorView()}>
            {(failure) => (
              <AdminRulesLoadError
                detail={adminApiErrorMessage(failure().code)}
                reloadGeneration={reloadGeneration}
              />
            )}
          </Match>
          <Match when={readyDocument()}>
            {(document) => (
              <AdminRulesWorkspace
                kind={kind}
                document={document()}
                target={target()}
                targetSignal={target}
                draft={draft}
                checkedCandidate={checkedCandidate}
                baseline={baseline}
                notice={notice}
                busy={busy}
                reloadGeneration={reloadGeneration}
                pending={pending}
                dirty={dirty}
              />
            )}
          </Match>
        </Switch>
      </section>
    </PendingCommandsContext>
  );
}

/** 文档读取失败时只展示稳定码文案，并推进世代以重新读取。 */
export function AdminRulesLoadError(props: {
  readonly detail: string;
  readonly reloadGeneration: Cell<bigint>;
}): JSX.Element {
  return (
    <AsyncState
      kind="error"
      title={props.detail}
      detail=""
      retryLabel={t("admin-rules-retry")}
      onRetry={() => advanceCommandReload(props.reloadGeneration)}
    />
  );
}
