// 仪表盘邮箱列表的分组编辑控件。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import { adminWorldLabel } from "../../presentation";
import {
  MAX_DASHBOARD_MAILBOXES,
  type RuntimeConfigForm,
  runtimeConfigFormAddMailbox,
  runtimeConfigFormMailboxes,
  runtimeConfigFormRemoveMailbox,
  runtimeConfigFormSetMailboxKey,
  runtimeConfigFormSetMailboxWorld,
} from "../model";
import { fieldId, mailboxCountError } from "../validation";
import { RuntimeSection } from "./fields";

/** 仪表盘邮箱分组；超过上限时保留全部行并提示修复。 */
export function MailboxesSection(props: {
  readonly form: Cell<RuntimeConfigForm>;
  readonly disabled: boolean;
  readonly onDirty: () => void;
}): JSX.Element {
  const mailboxes = createValueMemo(() => runtimeConfigFormMailboxes(props.form()));
  const canAddMailbox = createValueMemo(() => mailboxes().length < MAX_DASHBOARD_MAILBOXES);
  const tooManyMailboxes = createValueMemo(() => mailboxes().length > MAX_DASHBOARD_MAILBOXES);
  return (
    <RuntimeSection title={t("admin-runtime-section-mailboxes")} open={false}>
      <p class="admin-runtime-section-note">{t("admin-runtime-mailboxes-note")}</p>
      <Show when={tooManyMailboxes()}>
        <p class="admin-config-field-error" id="admin-runtime-mailbox-count-error">
          {mailboxCountError()}
        </p>
      </Show>
      {/* biome-ignore lint/a11y/useSemanticElements: 源码使用带 role="group" 的 div，改 fieldset 会改变 DOM 结构。 */}
      <div
        class="admin-runtime-mailboxes"
        id={fieldId("dashboard_mailboxes")}
        tabindex="-1"
        role="group"
        aria-label={t("admin-runtime-section-mailboxes")}
        aria-describedby={tooManyMailboxes() ? "admin-runtime-mailbox-count-error" : undefined}
      >
        <For each={mailboxes()} keyed={false}>
          {(mailbox, index) => (
            <div class="admin-runtime-mailbox">
              <label>
                <span>{t("admin-runtime-mailbox-world")}</span>
                <select
                  name={`mailboxes-${index}-world`}
                  autocomplete="off"
                  disabled={props.disabled}
                  value={mailbox().worldId}
                  onChange={(event) => {
                    props.form.update((current) =>
                      runtimeConfigFormSetMailboxWorld(current, index, event.currentTarget.value),
                    );
                    props.onDirty();
                  }}
                >
                  <option value="main">{adminWorldLabel("main")}</option>
                  <option value="isekai">{adminWorldLabel("isekai")}</option>
                </select>
              </label>
              <label>
                <span>{t("admin-runtime-mailbox-key")}</span>
                <input
                  type="text"
                  name={`mailboxes-${index}-key`}
                  autocomplete="off"
                  autocapitalize="none"
                  spellcheck="false"
                  disabled={props.disabled}
                  value={mailbox().mailboxKey}
                  onInput={(event) => {
                    props.form.update((current) =>
                      runtimeConfigFormSetMailboxKey(current, index, event.currentTarget.value),
                    );
                    props.onDirty();
                  }}
                />
              </label>
              <button
                class="admin-btn-secondary"
                type="button"
                disabled={props.disabled}
                onClick={() => {
                  props.form.update((current) => runtimeConfigFormRemoveMailbox(current, index));
                  props.onDirty();
                }}
              >
                {t("admin-runtime-mailbox-remove")}
              </button>
            </div>
          )}
        </For>
        <button
          class="admin-btn-secondary admin-runtime-mailbox-add"
          type="button"
          disabled={props.disabled || !canAddMailbox()}
          onClick={() => {
            props.form.update((current) => runtimeConfigFormAddMailbox(current));
            props.onDirty();
          }}
        >
          {t("admin-runtime-mailbox-add")}
        </button>
      </div>
    </RuntimeSection>
  );
}
