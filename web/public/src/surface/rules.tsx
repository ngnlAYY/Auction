// Public 站点文档页面：规则与关于共用同一套读取、语言跟随与 Markdown 排版。

import { createValueMemo } from "@auction/web-shared/reactive/memo";
import type { TaskResource } from "@auction/web-shared/reactive/resource";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import type { Result } from "@auction/web-shared/result";
import { MarkdownBlocks, withoutLeadingTitle } from "@auction/web-shared/ui/components/markdown";
import { currentLanguage, t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Show, useContext } from "solid-js";

import {
  DOCUMENT_UNPUBLISHED,
  fetchSiteDocument,
  type RulesDocument,
  type SiteDocument,
} from "../api/rules";
import { PublicRefreshGenerationContext } from "./board/refresh";
import { useContentLoading } from "./loading";

interface LoadedDocument {
  readonly requested: string;
  readonly result: Result<RulesDocument, string>;
}

/** 规则/关于页面；`kind` 由路由在挂载时固定，页面内部不切换文档种类。 */
export function page(kind: SiteDocument): JSX.Element {
  const refreshGeneration = useContext(PublicRefreshGenerationContext);
  // `currentLanguage()` 读取 context，是 hook；只能在组件渲染期调用，不能放进 resource 闭包。
  const locale = createValueMemo(() => currentLanguage());
  // 语言作为 resource 的显式依赖：语言切换时重新取回对应语言的正文，同时保留刷新代号触发的
  // 重试。请求被取消时返回 null，不把旧语言的响应当作新语言的结果缓存下来。
  const documentResource: TaskResource<LoadedDocument | null> = createTaskResource(
    // 刷新代号只需参与依赖；读取它保证 generation 变化会重跑本次请求。
    () => ({ generation: refreshGeneration(), requestedLocale: locale() }),
    async (dependencies, context) => {
      const result = await fetchSiteDocument(kind, dependencies.requestedLocale, context.signal);
      if (!context.alive()) {
        return null;
      }
      return { requested: dependencies.requestedLocale, result };
    },
  );
  // 重读保留的旧正文与错误只属于原语言，不能随新语言的页头一起展示。
  const current = createValueMemo(() => {
    const loaded = documentResource();
    if (loaded === null || loaded === undefined) {
      return null;
    }
    return loaded.requested === locale() ? loaded : null;
  });
  useContentLoading(() => current() === null);
  // 页面名称只作为区域的可访问名称；加载失败或尚未发布时没有文档标题可用。
  const pageName = createValueMemo(() =>
    kind === "rules" ? t("rules-title-fallback") : t("page-about-title"),
  );
  const readyDocument = createValueMemo(() => {
    const loaded = current();
    if (loaded === null || !loaded.result.ok) {
      return null;
    }
    return loaded.result.value;
  });
  const unpublished = createValueMemo(() => {
    const loaded = current();
    return loaded !== null && !loaded.result.ok && loaded.result.error === DOCUMENT_UNPUBLISHED;
  });

  return (
    <Show when={current() !== null} fallback={null}>
      <Show
        when={readyDocument()}
        fallback={
          <Show
            when={unpublished()}
            fallback={
              <article class="public-rules-page" aria-label={pageName()}>
                <div role="alert">
                  <h1>{t("rules-load-error-title")}</h1>
                  <p class="public-page-status">{t("rules-load-error-detail")}</p>
                  <button
                    class="public-state-retry"
                    type="button"
                    onClick={() => {
                      const generation = refreshGeneration.peek();
                      refreshGeneration.set(Math.min(generation + 1, Number.MAX_SAFE_INTEGER));
                    }}
                  >
                    {t("data-retry")}
                  </button>
                </div>
              </article>
            }
          >
            {/* “关于”是可选内容：尚未发布是确定的空状态，不提供重试。 */}
            <article class="public-rules-page" aria-label={pageName()}>
              <div class="public-auction-empty" role="status">
                <p class="public-auction-empty__title">{t("about-empty-title")}</p>
                <p class="public-auction-empty__hint">{t("about-empty-hint")}</p>
              </div>
            </article>
          </Show>
        }
      >
        {(document) => <SiteDocumentView document={document()} />}
      </Show>
    </Show>
  );
}

function SiteDocumentView(props: { readonly document: RulesDocument }): JSX.Element {
  return (
    // 文档标题与导航里的入口同名，只作为区域名称；正文从第一节开始。
    <article class="public-rules-page" aria-label={props.document.title}>
      <div class="public-rules-page__document">
        <MarkdownBlocks markdown={withoutLeadingTitle(props.document.markdown)} />
      </div>
    </article>
  );
}
