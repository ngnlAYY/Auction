// 单个邮件模板的两阶段编辑、即时校验与冲突恢复控件。

import type { AdminMailTemplateDto } from "@auction/contracts/dto/admin/mail-templates";
import { focusElementById } from "@auction/web-shared/platform/a11y";
import { sleep } from "@auction/web-shared/platform/timer";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";

import { mailTemplateBodyProblem } from "../../api/notification-templates";
import { formatAdminTime } from "../presentation";
import { placeholderList, problemMessage, templateLabel } from "./mail-template-labels";
import { TEMPLATE_ENTITY_PREFIX, type TemplateEditState } from "./mail-template-state";

function templateEntity(key: string): string {
  return `${TEMPLATE_ENTITY_PREFIX}${key}`;
}

function saveRevision(revision: number, published: number, rejected: boolean): number | null {
  return !rejected && revision === published ? revision : null;
}

/** 单个模板卡片；折叠状态由草稿、确认、冲突与未决命令共同决定。 */
export function MailTemplateEditor(props: {
  readonly template: AdminMailTemplateDto;
  readonly state: TemplateEditState;
}): JSX.Element {
  const tasks = createTaskScope();
  const key = props.template.key;
  const label = createValueMemo(() => templateLabel(key));
  const fieldId = `admin-mail-template-${key}`;
  const helpId = `${fieldId}-help`;
  const errorId = `${fieldId}-error`;
  const showProblem = createCell(false);

  const occupied = createValueMemo(() => props.state.pending.contains(templateEntity(key)));
  const cardBusy = createValueMemo(() => props.state.pending.containsLocal(TEMPLATE_ENTITY_PREFIX));
  const ownConfirmation = createValueMemo(() => {
    const pending = props.state.confirmation();
    return pending !== null && pending.key === key ? pending : null;
  });
  const confirming = createValueMemo(() => ownConfirmation() !== null);
  const busy = createValueMemo(
    () => occupied() || cardBusy() || props.state.confirmation() !== null,
  );
  const listReady = createValueMemo(() => props.state.listReady());
  const draft = createValueMemo(() => props.state.drafts().get(key) ?? null);
  const dirty = createValueMemo(() => draft() !== null);
  const rejected = createValueMemo(() => props.state.conflicts().has(key));
  const conflicted = createValueMemo(() => {
    const current = draft();
    const draftConflicted =
      current === null
        ? rejected()
        : saveRevision(current.revision, props.template.revision, rejected()) === null;
    const own = ownConfirmation();
    return draftConflicted || (own !== null && own.expectedRevision !== props.template.revision);
  });
  const body = createValueMemo(() => draft()?.body ?? props.template.body);
  const visibleProblem = createValueMemo(() => {
    if (!showProblem() || !dirty() || conflicted()) {
      return null;
    }
    return mailTemplateBodyProblem(props.template, body());
  });
  const justAccepted = createValueMemo(() => props.state.accepted() === key);
  const expanded = createValueMemo(
    () => dirty() || confirming() || conflicted() || justAccepted() || occupied(),
  );
  const status = createValueMemo(() => {
    if (!props.template.customized) {
      return t("admin-mail-template-status-default");
    }
    return props.template.usable
      ? t("admin-mail-template-status-custom")
      : t("admin-mail-template-status-unusable");
  });
  const pmIdentity = key === "lot_watch_pm";
  const rejectionReason = key === "auction_equipment_rejected";
  const watchBody = key === "lot_watch" || key === "lot_watch_pm";
  const requiredPlaceholders = createValueMemo(() => {
    const required = [...props.template.required_placeholders];
    if (pmIdentity && !required.includes("notification_id")) {
      required.push("notification_id");
    }
    return required;
  });
  const optionalPlaceholders = createValueMemo(() => {
    const required = requiredPlaceholders();
    return props.template.allowed_placeholders.filter(
      (name) =>
        !required.includes(name) &&
        !(rejectionReason && (name === "reason_code" || name === "reason_text")),
    );
  });
  const description = createValueMemo(() =>
    visibleProblem() !== null ? `${helpId} ${errorId}` : helpId,
  );
  const revisionNote = createValueMemo(() => {
    if (props.template.updated_by === null || props.template.updated_at === null) {
      return null;
    }
    return t("admin-mail-template-revision", {
      revision: props.template.revision,
      actor: props.template.updated_by,
      time: formatAdminTime(props.template.updated_at),
    });
  });

  const focusBodyAfterRender = (): void => {
    tasks.spawn(async (context) => {
      if (!(await sleep(0, context.signal)) || !context.alive()) {
        return;
      }
      focusElementById(fieldId);
    });
  };

  const edit = (event: InputEvent & { currentTarget: HTMLTextAreaElement }): void => {
    if (!props.state.canEdit(key, props.template.revision)) {
      return;
    }
    const value = event.currentTarget.value;
    const current = props.state.drafts.peek().get(key);
    if (current !== undefined && current.revision !== props.template.revision) {
      return;
    }
    props.state.accepted.set(null);
    // 只在同一基准版本下把改回原文视为撤销编辑。
    if (value === props.template.body) {
      props.state.drafts.update((drafts) => {
        const next = new Map(drafts);
        next.delete(key);
        return next;
      });
      showProblem.set(false);
    } else {
      props.state.drafts.update((drafts) => {
        const next = new Map(drafts);
        next.set(key, { revision: props.template.revision, body: value });
        return next;
      });
    }
  };

  const beginSave = (event: SubmitEvent): void => {
    event.preventDefault();
    const invalid = props.state.beginSave(props.template) !== null;
    showProblem.set(invalid);
    if (invalid) {
      focusBodyAfterRender();
    }
  };

  const beginReset = (): void => {
    const current = props.state.drafts.peek().get(key);
    if (
      !props.state.canEdit(key, props.template.revision) ||
      !props.state.listReady() ||
      (current !== undefined && current.revision !== props.template.revision)
    ) {
      return;
    }
    props.state.accepted.set(null);
    props.state.confirmation.set({
      key,
      expectedRevision: props.template.revision,
      body: null,
    });
  };

  const discard = (): void => {
    showProblem.set(false);
    props.state.discard(key, conflicted());
  };

  return (
    <details class="admin-runtime-section admin-mail-template" open={expanded()}>
      <summary>
        <span id={`${fieldId}-label`}>{label()}</span>
        <span class="admin-mail-template__status">{status()}</span>
      </summary>
      <form
        class="admin-runtime-section__body"
        action={`/api/v1/admin/mail-templates/${key}`}
        method="post"
        aria-labelledby={`${fieldId}-label`}
        novalidate
        onSubmit={beginSave}
      >
        <Show when={revisionNote()}>
          {(note) => <p class="admin-runtime-section-note">{note()}</p>}
        </Show>
        <div id={helpId}>
          <p class="admin-runtime-section-note">
            <Show
              when={props.template.allowed_placeholders.length > 0}
              fallback={t("admin-mail-template-no-placeholders")}
            >
              {t("admin-mail-template-required")}: {placeholderList(requiredPlaceholders())} ·{" "}
              {t("admin-mail-template-optional")}: {placeholderList(optionalPlaceholders())}
            </Show>
          </p>
          <Show when={pmIdentity}>
            <p class="admin-runtime-section-note">
              {problemMessage("missing_notification_identity")}
            </p>
          </Show>
          <Show when={rejectionReason}>
            <p class="admin-runtime-section-note">{problemMessage("missing_rejection_reason")}</p>
          </Show>
          <Show when={watchBody}>
            <p class="admin-runtime-section-note">{t("admin-mail-template-watch-limit")}</p>
          </Show>
        </div>
        <div class="admin-runtime-field admin-runtime-field--template">
          <label for={fieldId}>
            {label()}
            <span aria-hidden="true"> *</span>
          </label>
          <textarea
            id={fieldId}
            name="body"
            autocomplete="off"
            spellcheck="false"
            rows="8"
            disabled={busy() && !conflicted()}
            readonly={conflicted()}
            required
            aria-invalid={visibleProblem() !== null ? "true" : "false"}
            aria-describedby={description()}
            value={body()}
            onInput={edit}
          />
        </div>
        <Show when={visibleProblem()}>
          {(problem) => (
            <p class="admin-runtime-section-warning" id={errorId}>
              {problemMessage(problem())}
            </p>
          )}
        </Show>
        <Show
          when={conflicted()}
          fallback={
            <Show
              when={ownConfirmation()}
              fallback={
                <div class="admin-mail-template__actions">
                  <button
                    class="admin-btn-primary"
                    type="submit"
                    disabled={busy() || !listReady() || !dirty()}
                  >
                    {t("admin-mail-template-save")}
                  </button>
                  <button
                    class="admin-btn-secondary"
                    type="button"
                    disabled={busy() || !dirty()}
                    onClick={discard}
                  >
                    {t("admin-mail-template-discard")}
                  </button>
                  <button
                    class="admin-btn-secondary"
                    type="button"
                    disabled={busy() || !listReady() || !props.template.customized}
                    onClick={beginReset}
                  >
                    {t("admin-mail-template-reset")}
                  </button>
                </div>
              }
            >
              {(pending) => (
                <>
                  <p>
                    {pending().body !== null
                      ? t("admin-mail-template-confirm-save")
                      : t("admin-mail-template-confirm-reset")}
                  </p>
                  <div class="admin-mail-template__actions">
                    <button
                      class="admin-btn-primary"
                      type="button"
                      disabled={occupied() || cardBusy() || !listReady()}
                      onClick={() => props.state.submit(pending())}
                    >
                      {t("admin-mail-template-confirm")}
                    </button>
                    <button
                      class="admin-btn-secondary"
                      type="button"
                      onClick={() => props.state.confirmation.set(null)}
                    >
                      {t("admin-mail-template-cancel")}
                    </button>
                  </div>
                </>
              )}
            </Show>
          }
        >
          <p class="admin-runtime-section-warning" role="status">
            {t("admin-mail-template-draft-conflict")}
          </p>
          <button
            class="admin-btn-secondary"
            type="button"
            disabled={occupied() || cardBusy() || (busy() && !confirming())}
            onClick={discard}
          >
            {t("admin-mail-template-discard-reload")}
          </button>
        </Show>
      </form>
    </details>
  );
}
