// Admin 邮件轮询运行观测；明确区分已完成游标、当前目标和最近失败，不提供跳过或重发动作。

import type { AdminMailPollingDiagnosticsResponse } from "@auction/contracts/dto/admin/mail/progress";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Match, Show, Switch } from "solid-js";

import { formatAdminTime } from "../presentation";

/** 轮询卡片的诊断扩展；`null` 表示本次读取没有返回诊断。 */
export interface MailPollingProgressProps {
  readonly diagnostics: AdminMailPollingDiagnosticsResponse | null;
}

/** 邮件的已完成游标、当前处理目标与最近失败；未加载诊断时只显示占位文案。 */
export function MailPollingProgress(props: MailPollingProgressProps): JSX.Element {
  const diagnostics = createValueMemo(() => props.diagnostics);
  // `available` 才展示游标值；空串是合法回显（缺少 MID 时源码用 `unwrap_or_default`）。
  const cursorCode = createValueMemo((): string | null => {
    const current = diagnostics();
    return current !== null && current.cursor_status === "available"
      ? (current.completed_external_mail_id ?? "")
      : null;
  });
  const cursorText = createValueMemo((): string | null => {
    const current = diagnostics();
    if (current === null || current.cursor_status === "available") {
      return null;
    }
    return current.cursor_status === "empty"
      ? t("admin-mail-progress-cursor-empty")
      : t("admin-mail-progress-cursor-hidden");
  });
  const workerUnavailable = createValueMemo(() => {
    const current = diagnostics();
    return current !== null && current.worker_condition !== "available";
  });
  // 只有 Worker 可观测时才展示进程内进度；未观测到进度与 Worker 不可用是两种状态。
  const progress = createValueMemo(() => {
    const current = diagnostics();
    return current !== null && current.worker_condition === "available" ? current.progress : null;
  });
  return (
    <Show when={diagnostics()} fallback={<p>{t("admin-mail-progress-not-loaded")}</p>}>
      {(current) => (
        <div class="admin-mail-progress">
          <h4>{t("admin-mail-progress-title")}</h4>
          <p class="admin-mail-progress__hint">{t("admin-mail-progress-note")}</p>
          <dl>
            <div>
              <dt>{t("admin-mail-progress-cursor")}</dt>
              <dd>
                <Switch fallback={null}>
                  <Match when={cursorCode() !== null}>
                    <code>{cursorCode() ?? ""}</code>
                  </Match>
                  <Match when={cursorText() !== null}>{cursorText() ?? ""}</Match>
                </Switch>
              </dd>
            </div>
            <div>
              <dt>{t("admin-mail-progress-observed")}</dt>
              <dd>{formatAdminTime(current().observed_at)}</dd>
            </div>
          </dl>
          <Switch fallback={<p>{t("admin-mail-progress-not-observed")}</p>}>
            <Match when={workerUnavailable()}>
              <p>{t("admin-mail-progress-unavailable")}</p>
            </Match>
            <Match when={progress()}>
              {(running) => (
                <>
                  <dl>
                    <div>
                      <dt>{t("admin-mail-progress-stage")}</dt>
                      <dd>{stageLabel(running().stage)}</dd>
                    </div>
                    <div>
                      <dt>{t("admin-mail-progress-current")}</dt>
                      <dd>
                        <Show
                          when={running().external_mail_id !== null}
                          fallback={t("admin-mail-progress-no-mid")}
                        >
                          <code>{running().external_mail_id ?? ""}</code>
                        </Show>
                      </dd>
                    </div>
                    <div>
                      <dt>{t("admin-mail-progress-updated")}</dt>
                      <dd>{formatAdminTime(running().updated_at)}</dd>
                    </div>
                    <div>
                      <dt>{t("admin-mail-progress-worker-observed")}</dt>
                      <dd>{formatAdminTime(current().worker_observed_at ?? "")}</dd>
                    </div>
                  </dl>
                  <Show
                    when={running().last_failure}
                    fallback={<p>{t("admin-mail-progress-no-failure")}</p>}
                  >
                    {(failure) => (
                      <dl>
                        <div>
                          <dt>{t("admin-mail-progress-failed-mid")}</dt>
                          <dd>
                            <Show
                              when={failure().external_mail_id !== null}
                              fallback={t("admin-mail-progress-no-mid")}
                            >
                              <code>{failure().external_mail_id ?? ""}</code>
                            </Show>
                          </dd>
                        </div>
                        <div>
                          <dt>{t("admin-mail-progress-failed-stage")}</dt>
                          <dd>{stageLabel(failure().stage)}</dd>
                        </div>
                        <div>
                          <dt>{t("admin-mail-progress-failed-at")}</dt>
                          <dd>{formatAdminTime(failure().occurred_at)}</dd>
                        </div>
                      </dl>
                    )}
                  </Show>
                </>
              )}
            </Match>
          </Switch>
        </div>
      )}
    </Show>
  );
}

/** 入站阶段说明；未知阶段按「暂不可用」展示，与源码的兜底分支一致。 */
function stageLabel(stage: string): string {
  switch (stage) {
    case "fetching":
      return t("admin-mail-progress-fetching");
    case "reading_detail":
      return t("admin-mail-progress-reading-detail");
    case "processing":
      return t("admin-mail-progress-processing");
    case "committing":
      return t("admin-mail-progress-committing");
    case "idle":
      return t("admin-mail-progress-idle");
    case "failed":
      return t("admin-mail-progress-failed");
    case "canceled":
      return t("admin-mail-progress-canceled");
    default:
      return t("admin-mail-progress-unavailable");
  }
}
