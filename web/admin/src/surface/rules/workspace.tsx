// 站点文档（规则、关于）草稿编辑、校验和发布 workspace。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { err, ok, unreachable } from "@auction/web-shared/result";
import { Dialog } from "@auction/web-shared/ui/components/dialog";
import { Time } from "@auction/web-shared/ui/components/time";
import { SUPPORTED_LOCALES, t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { type Accessor, For, Show } from "solid-js";

import { adminApiFailureClient, adminApiFailureIsConflict } from "../../api/failure";
import {
  type AdminDocumentKind,
  type AdminRules,
  type AdminRulesCandidate,
  adminDocumentKindPendingKey,
  prepareAdminRulesPublish,
  publishAdminRules,
  validateAdminRules,
} from "../../api/rules";
import { useAdminSessionHost } from "../auth/shell";
import { commandDeliveryReported, type PendingCommands, setIfAlive } from "../pending";
import { advanceCommandReload } from "../pending/sources";
import { DEFAULT_RULES_LOCALE, noticeText, toDraft } from "./draft";
import {
  type RuleNotice,
  type RulesBaseline,
  type RulesTransition,
  rulesBaselineMatchesCandidate,
  rulesBaselineMatchesDocument,
} from "./model";
import { AdminRulesPreview } from "./preview";

/** 规则编辑器与所属认证会话共享的门禁、草稿与读取世代。 */
export interface AdminRulesWorkspaceProps {
  readonly kind: AdminDocumentKind;
  readonly document: AdminRules;
  readonly target: string;
  readonly targetSignal: Cell<string>;
  readonly draft: Cell<AdminRulesCandidate | null>;
  readonly checkedCandidate: Cell<AdminRulesCandidate | null>;
  readonly baseline: Cell<RulesBaseline | null>;
  readonly notice: Cell<RuleNotice | null>;
  readonly busy: Cell<boolean>;
  readonly reloadGeneration: Cell<bigint>;
  readonly pending: PendingCommands;
  readonly dirty: Accessor<boolean>;
}

/** 展示规则编辑器，并确保只有最近一次已校验草稿可以发布。 */
export function AdminRulesWorkspace(props: AdminRulesWorkspaceProps): JSX.Element {
  const host = useAdminSessionHost();
  const frozen = (): boolean =>
    props.busy() || props.pending.contains(adminDocumentKindPendingKey(props.kind));
  const transitionConfirmation = createCell<RulesTransition | null>(null);
  const currentDraft = createValueMemo(
    (): AdminRulesCandidate | null =>
      props.draft() ??
      (props.document.revision === 0 ? toDraft(props.document, props.target) : null),
  );
  const conflict = createValueMemo((): boolean => {
    const base = props.baseline();
    return base === null || !rulesBaselineMatchesDocument(base, props.document);
  });
  const canPublish = createValueMemo((): boolean => {
    const candidate = currentDraft();
    return (
      !frozen() &&
      !conflict() &&
      props.dirty() &&
      sameCandidate(props.checkedCandidate(), candidate)
    );
  });

  // 修改目标或重载只在用户确认丢弃后执行；校验和已提交命令仍共用原门禁。
  const applyTransition = (transition: RulesTransition): void => {
    if (frozen()) {
      return;
    }
    switch (transition.kind) {
      case "select":
        if (transition.locale === props.targetSignal.peek()) {
          transitionConfirmation.set(null);
          return;
        }
        props.draft.set(null);
        props.baseline.set(null);
        props.checkedCandidate.set(null);
        props.notice.set(null);
        transitionConfirmation.set(null);
        props.targetSignal.set(transition.locale);
        return;
      case "reload":
        props.draft.set(null);
        props.baseline.set(null);
        props.checkedCandidate.set(null);
        props.notice.set(null);
        transitionConfirmation.set(null);
        advanceCommandReload(props.reloadGeneration);
        return;
      default:
        unreachable(transition);
    }
  };
  const requestTransition = (transition: RulesTransition): void => {
    if (frozen()) {
      return;
    }
    // 重新选择当前语言不能清空当前草稿。
    const sameTarget =
      transition.kind === "select" && transition.locale === props.targetSignal.peek();
    if (sameTarget) {
      applyTransition(transition);
      return;
    }
    const candidate = props.draft.peek();
    const base = props.baseline.peek();
    const hasEdits =
      candidate !== null && (base === null || !rulesBaselineMatchesCandidate(base, candidate));
    if (hasEdits) {
      transitionConfirmation.set(transition);
    } else {
      applyTransition(transition);
    }
  };

  const submitValidation = (event: SubmitEvent): void => {
    event.preventDefault();
    const candidate = currentDraft();
    if (
      candidate === null ||
      frozen() ||
      conflict() ||
      rustTrim(candidate.title).length === 0 ||
      !sameCandidate(props.draft.peek(), candidate)
    ) {
      return;
    }
    const requestedCandidate: AdminRulesCandidate = {
      locale: candidate.locale,
      title: candidate.title,
      markdown: candidate.markdown,
    };
    const locale = requestedCandidate.locale;
    const title = requestedCandidate.title;
    const markdown = requestedCandidate.markdown;
    props.busy.set(true);
    props.notice.set(null);
    // 校验跟随 busy 所属的认证会话作用域：站内切页不取消，回执也只回写同一次编辑。
    props.pending.host.tasks.spawn(async (context) => {
      const result = await validateAdminRules(
        host,
        props.kind,
        locale,
        title,
        markdown,
        context.signal,
      ).catch(() => {
        // 校验接口用 Result 表达完成结果；Promise 拒绝是传输或程序边界异常，
        // 转成稳定错误码后仍按正常回执收尾，不能锁住 busy 或把异常原文交给全局钩子。
        return err(adminApiFailureClient("network_error"));
      });
      if (!context.alive()) {
        return;
      }
      // 校验只可回写发起时的语言与草稿，不能覆盖已切换的编辑目标。
      if (
        props.targetSignal.peek() !== locale ||
        !sameCandidate(props.draft.peek(), requestedCandidate)
      ) {
        props.busy.set(false);
        return;
      }
      if (result.ok) {
        props.checkedCandidate.set(result.value);
        props.draft.set(result.value);
        props.notice.set({ kind: "validated" });
      } else {
        props.notice.set(
          adminApiFailureIsConflict(result.error)
            ? { kind: "conflict" }
            : { kind: "action-error", code: result.error.code },
        );
      }
      props.busy.set(false);
    });
  };

  const publish = (): void => {
    const publishValue = currentDraft();
    if (
      publishValue === null ||
      frozen() ||
      conflict() ||
      props.targetSignal() !== publishValue.locale ||
      !sameCandidate(props.checkedCandidate.peek(), publishValue) ||
      !sameCandidate(props.draft.peek(), publishValue)
    ) {
      return;
    }
    const candidate: AdminRulesCandidate = {
      locale: publishValue.locale,
      title: publishValue.title,
      markdown: publishValue.markdown,
    };
    const revision = props.document.revision;
    // 正文在提交时冻结；同一次命令的每次原样重试都复用同一份输入。
    const prepare = prepareAdminRulesPublish(
      revision,
      candidate.locale,
      candidate.title,
      candidate.markdown,
    );
    props.notice.set(null);
    props.pending.submitWithReceipt(
      adminDocumentKindPendingKey(props.kind),
      t("admin-rules-publish"),
      async (commandId, signal) => {
        const result = await publishAdminRules(host, props.kind, commandId, prepare, signal);
        return result.ok ? ok(null) : err(result.error.code);
      },
      (_value, sourceAlive) => {
        const alive = (): boolean => props.pending.host.alive();
        // 清理会话草稿时废弃同世代的读取，避免重挂载的编辑器停在加载或使用旧基线。
        setIfAlive(props.draft, null, alive);
        setIfAlive(props.baseline, null, alive);
        setIfAlive(props.checkedCandidate, null, alive);
        advanceCommandReload(props.reloadGeneration);
        if (sourceAlive) {
          setIfAlive(props.notice, { kind: "published" }, alive);
        }
        return commandDeliveryReported(sourceAlive, () => null);
      },
      (code) => {
        setIfAlive(props.notice, { kind: "action-error", code }, () => props.pending.host.alive());
      },
    );
  };

  const editDraft = (edit: (candidate: AdminRulesCandidate) => AdminRulesCandidate): void => {
    if (frozen() || props.targetSignal.peek() !== props.target) {
      return;
    }
    const current = props.draft.peek();
    const candidate =
      current ??
      ({
        locale: props.target,
        title: "",
        markdown: "",
      } satisfies AdminRulesCandidate);
    props.draft.set(edit(candidate));
    props.checkedCandidate.set(null);
    props.notice.set(null);
  };

  const visibleNotice = createValueMemo((): RuleNotice | null => {
    const current = props.notice();
    return current !== null && current.kind !== "published" ? current : null;
  });

  return (
    <>
      {/* biome-ignore lint/a11y/useSemanticElements: 源码使用带 role="group" 的 div，改 fieldset 会改变 DOM 结构。 */}
      <div class="admin-rules-tabs" role="group" aria-label={t("admin-rules-languages")}>
        <For each={SUPPORTED_LOCALES} keyed={(locale) => locale.code}>
          {(locale) => (
            <button
              type="button"
              lang={locale().code}
              aria-pressed={locale().code === props.target ? "true" : "false"}
              disabled={frozen()}
              onClick={() => requestTransition({ kind: "select", locale: locale().code })}
            >
              {locale().label}
            </button>
          )}
        </For>
      </div>
      <label class="admin-rules-default" for="rules-default-locale">
        {t("admin-rules-default-locale")}
      </label>{" "}
      <select
        class="admin-rules-default-locale"
        disabled={true}
        id="rules-default-locale"
        name="default_locale"
        value={DEFAULT_RULES_LOCALE}
      >
        <option value={DEFAULT_RULES_LOCALE}>{DEFAULT_RULES_LOCALE}</option>
      </select>
      <div class="admin-rules-facts">
        <span>{t("admin-rules-revision", { revision: props.document.revision })}</span>
        <span>{t("admin-rules-published", { actor: props.document.published_by })}</span>
        <span>
          {t("admin-rules-published-at")}
          <Time value={props.document.published_at} />
        </span>
      </div>
      <Show when={conflict()}>
        <p class="admin-rules-error" role="alert">
          {t("admin-rules-draft-conflict")}
        </p>
      </Show>
      <Show when={currentDraft()}>
        {(value) => (
          <form class="admin-rules-form" onSubmit={submitValidation}>
            <div class="admin-rules-editor">
              <div class="admin-rules-fields">
                <label for="rules-title-input">{t("admin-rules-title")}</label>
                <input
                  id="rules-title-input"
                  name="title"
                  autocomplete="off"
                  required={true}
                  type="text"
                  value={value().title}
                  disabled={frozen()}
                  onInput={(event) => {
                    const nextTitle = event.currentTarget.value;
                    editDraft((candidate) => ({ ...candidate, title: nextTitle }));
                  }}
                />
                <label for="rules-markdown">{t("admin-rules-markdown")}</label>
                <textarea
                  id="rules-markdown"
                  name="markdown"
                  autocomplete="off"
                  spellcheck="false"
                  value={value().markdown}
                  disabled={frozen()}
                  onInput={(event) => {
                    const nextMarkdown = event.currentTarget.value;
                    editDraft((candidate) => ({ ...candidate, markdown: nextMarkdown }));
                  }}
                />
              </div>
              <AdminRulesPreview draft={value()} />
            </div>

            <Show when={visibleNotice()}>
              {(currentNotice) => (
                <p
                  class={
                    currentNotice().kind === "validated" ? "admin-rules-ok" : "admin-rules-error"
                  }
                  role={currentNotice().kind === "validated" ? "status" : "alert"}
                >
                  {noticeText(currentNotice())}
                </p>
              )}
            </Show>

            <div class="admin-rules-actions">
              <button
                type="button"
                disabled={frozen()}
                onClick={() => requestTransition({ kind: "reload" })}
              >
                {t("admin-rules-reload")}
              </button>
              <button
                type="submit"
                disabled={frozen() || conflict() || rustTrim(value().title).length === 0}
              >
                {props.busy() ? t("admin-action-working") : t("admin-rules-validate")}
              </button>
              <button type="button" disabled={!canPublish()} onClick={publish}>
                {props.document.revision === 0 ? t("admin-rules-create") : t("admin-rules-publish")}
              </button>
            </div>
          </form>
        )}
      </Show>
      <Show when={transitionConfirmation()}>
        {(transition) => (
          <Dialog
            title={t("admin-rules-discard-title")}
            closeLabel={t("admin-runtime-keep-editing")}
            onClose={() => transitionConfirmation.set(null)}
          >
            {transitionDetail(transition())}
            <div class="admin-shell__dialog-actions">
              <button
                class="admin-btn-secondary"
                type="button"
                autofocus={true}
                onClick={() => transitionConfirmation.set(null)}
              >
                {t("admin-runtime-keep-editing")}
              </button>
              <button
                class="admin-btn-danger"
                type="button"
                disabled={frozen()}
                onClick={() => applyTransition(transition())}
              >
                {t("admin-rules-discard-confirm")}
              </button>
            </div>
          </Dialog>
        )}
      </Show>
    </>
  );
}

/** 丢弃确认的正文随待执行的下一步变化，不携带发布命令或草稿正文。 */
function transitionDetail(transition: RulesTransition): JSX.Element {
  switch (transition.kind) {
    case "select":
      return <p>{t("admin-rules-discard-switch-detail", { locale: transition.locale })}</p>;
    case "reload":
      return <p>{t("admin-rules-discard-reload-detail")}</p>;
    default:
      return unreachable(transition);
  }
}

/** Rust 对 `Option<AdminRulesCandidate>` 的 `==` 按字段比较，而不是比较对象身份。 */
function sameCandidate(
  left: AdminRulesCandidate | null,
  right: AdminRulesCandidate | null,
): boolean {
  if (left === null || right === null) {
    return left === right;
  }
  return (
    left.locale === right.locale && left.title === right.title && left.markdown === right.markdown
  );
}

/** 与 Rust `str::trim` 的字符集合一致的裁剪。 */
function rustTrim(value: string): string {
  let start = 0;
  let end = value.length;
  while (start < end && isRustWhitespace(value.charCodeAt(start))) {
    start += 1;
  }
  while (end > start && isRustWhitespace(value.charCodeAt(end - 1))) {
    end -= 1;
  }
  return value.slice(start, end);
}

function isRustWhitespace(code: number): boolean {
  return (
    (code >= 0x09 && code <= 0x0d) ||
    code === 0x20 ||
    code === 0x85 ||
    code === 0xa0 ||
    code === 0x1680 ||
    (code >= 0x2000 && code <= 0x200a) ||
    code === 0x2028 ||
    code === 0x2029 ||
    code === 0x202f ||
    code === 0x205f ||
    code === 0x3000
  );
}
