// 邮件模板卡片：出站通知正文保存在数据库，与 runtime 文件配置无关。逐个模板编辑，两阶段
// 确认后保存或恢复内置默认正文；结果未知时只允许原样重试。

import type { AdminMailTemplateDto } from "@auction/contracts/dto/admin/mail-templates";
import type { Cell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { err, type Result } from "@auction/web-shared/result";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import { fetchMailTemplates } from "../../api/notification-templates";
import { useAdminSessionHost } from "../auth/shell";
import { PendingCommandsContext } from "../pending";
import { PendingCommandPanel } from "../pending/panel";
import { createCommandReload } from "../pending/sources";
import { adminApiErrorMessage } from "../presentation";
import { MailTemplateEditor } from "./mail-template-editor";
import { templateLabel } from "./mail-template-labels";
import { type TemplateListRead, useTemplateEditState } from "./mail-template-state";

const U64_MAX = 18_446_744_073_709_551_615n;

type TemplateListResult = Result<readonly AdminMailTemplateDto[], string>;

function advanceRefresh(refresh: Cell<bigint>): void {
  const current = refresh.peek();
  refresh.set(current === U64_MAX ? current : current + 1n);
}

/** 邮件模板编辑卡；模板读取、模板操作和成功提示共享同一份卡片状态。 */
export function MailTemplateCard(): JSX.Element {
  const host = useAdminSessionHost();
  const refresh = createCommandReload(0n);
  const snapshot = createTaskResource(
    () => refresh(),
    async (generation, context): Promise<TemplateListRead> => {
      let result: TemplateListResult;
      try {
        result = await fetchMailTemplates(host, context.signal);
      } catch {
        // 传输层异常可能含秘密；只按安全的传输失败码收尾。
        result = err("network_error");
      }
      return [generation, result];
    },
  );
  const state = useTemplateEditState(host, snapshot, refresh);
  const current = createValueMemo(() => snapshot());
  const templates = createValueMemo<readonly AdminMailTemplateDto[] | null>(() => {
    const loaded = current();
    return loaded?.[1].ok ? loaded[1].value : null;
  });
  const errorCode = createValueMemo<string | null>(() => {
    const loaded = current();
    return loaded !== undefined && !loaded[1].ok ? loaded[1].error : null;
  });

  return (
    <PendingCommandsContext value={state.pending}>
      <section class="admin-mail-templates admin-panel">
        <h2>{t("admin-runtime-section-templates")}</h2>
        <p class="admin-panel__note">{t("admin-mail-templates-note")}</p>
        <button class="admin-btn-secondary" type="button" onClick={() => advanceRefresh(refresh)}>
          {t("admin-mail-templates-refresh")}
        </button>
        <PendingCommandPanel owner={state.pending} />
        <div role="status">
          <Show when={state.accepted()}>
            {(key) => (
              <p class="admin-shell__banner">
                {templateLabel(key())} · {t("admin-mail-template-done")}
              </p>
            )}
          </Show>
        </div>
        <Show
          when={current()}
          fallback={
            <AsyncState kind="loading" title={t("admin-mail-templates-loading")} detail="" />
          }
        >
          <Show
            when={templates()}
            fallback={
              <AsyncState kind="error" title={adminApiErrorMessage(errorCode() ?? "")} detail="" />
            }
          >
            {(items) => (
              <div class="admin-mail-templates__list">
                <For each={items()} keyed={(template) => template.key}>
                  {(template) => <MailTemplateEditor template={template()} state={state} />}
                </For>
              </div>
            )}
          </Show>
        </Show>
      </section>
    </PendingCommandsContext>
  );
}
