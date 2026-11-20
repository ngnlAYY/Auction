// 监听器模式与地址配置控件。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";

import { type RuntimeConfigForm, runtimeConfigFormInput } from "../model";
import { RuntimeSection, SelectField, TextField } from "./fields";

/** 监听器分组；TCP 只编辑地址，Unix 套接字编辑分组、路径与运行目录。 */
export function ListenerSection(props: {
  readonly form: Cell<RuntimeConfigForm>;
  readonly disabled: boolean;
  readonly onDirty: () => void;
}): JSX.Element {
  const listenerMode = createValueMemo(() => runtimeConfigFormInput(props.form(), "listener.mode"));
  return (
    <RuntimeSection title={t("admin-runtime-section-listener")} open={false}>
      <p class="admin-runtime-section-warning">{t("admin-runtime-listener-warning")}</p>
      <div class="admin-runtime-field-grid">
        <SelectField
          form={props.form}
          disabled={props.disabled}
          path="listener.mode"
          label={t("admin-runtime-listener-mode")}
          options={[
            ["tcp", t("admin-runtime-listener-tcp")],
            ["unix", t("admin-runtime-listener-unix")],
          ]}
          onDirty={props.onDirty}
        />
        <Show
          when={listenerMode() === "tcp"}
          fallback={
            <>
              <TextField
                form={props.form}
                disabled={props.disabled}
                path="listener.group"
                label={t("admin-runtime-listener-group")}
                onDirty={props.onDirty}
              />
              <TextField
                form={props.form}
                disabled={props.disabled}
                path="listener.socket_path"
                label={t("admin-runtime-listener-socket")}
                onDirty={props.onDirty}
              />
              <TextField
                form={props.form}
                disabled={props.disabled}
                path="listener.runtime_directory"
                label={t("admin-runtime-listener-directory")}
                onDirty={props.onDirty}
              />
            </>
          }
        >
          <TextField
            form={props.form}
            disabled={props.disabled}
            path="listener.address"
            label={t("admin-runtime-listener-address")}
            onDirty={props.onDirty}
          />
        </Show>
      </div>
    </RuntimeSection>
  );
}
