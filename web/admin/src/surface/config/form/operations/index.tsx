// 分组接管运行参数的编辑器；未接管时明确显示继承状态，不伪造生效值。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";

import { type RuntimeConfigForm, runtimeConfigFormBoolean } from "../../model";
import { RuntimeSection, SwitchField } from "..";
import { AlertsFields } from "./alerts";
import { EconomicsFields } from "./economics";
import { ForumFields } from "./forum";
import { MailFields } from "./mail";
import { ScheduleFields } from "./schedule";
import { WorkerFields } from "./worker";

/** 运维相关分组的入口；与源码一致，先提示热更新字段再按固定顺序排列各组。 */
export function OperationalConfigFields(props: {
  readonly form: Cell<RuntimeConfigForm>;
  readonly disabled: boolean;
  readonly onDirty: () => void;
}): JSX.Element {
  return (
    <>
      <p class="admin-runtime-section-warning">{t("admin-runtime-hot-fields-note")}</p>
      <MailFields form={props.form} disabled={props.disabled} onDirty={props.onDirty} />
      <ForumFields form={props.form} disabled={props.disabled} onDirty={props.onDirty} />
      <EconomicsFields form={props.form} disabled={props.disabled} onDirty={props.onDirty} />
      <ScheduleFields form={props.form} disabled={props.disabled} onDirty={props.onDirty} />
      <AlertsFields form={props.form} disabled={props.disabled} onDirty={props.onDirty} />
      <WorkerFields form={props.form} disabled={props.disabled} onDirty={props.onDirty} />
    </>
  );
}

/** 可接管的分组；未接管时只显示继承提示，接管后才渲染该组的字段。 */
export function ManagedSection(props: {
  readonly form: Cell<RuntimeConfigForm>;
  readonly disabled: boolean;
  readonly path: string;
  readonly title: string;
  readonly note: string;
  readonly onDirty: () => void;
  readonly children: JSX.Element;
}): JSX.Element {
  const managed = createValueMemo(() => runtimeConfigFormBoolean(props.form(), props.path));
  return (
    <RuntimeSection title={props.title} open={false}>
      <SwitchField
        form={props.form}
        disabled={props.disabled}
        path={props.path}
        label={t("admin-runtime-op-managed")}
        onDirty={props.onDirty}
      />
      <Show
        when={managed()}
        fallback={<p class="admin-runtime-section-note">{t("admin-runtime-op-inherited")}</p>}
      >
        <p class="admin-runtime-section-note">
          {t("admin-runtime-op-managed-note", {
            hot_reload_note: t("admin-runtime-hot-fields-note"),
          })}
        </p>
        <p class="admin-runtime-section-note">{props.note}</p>
        {props.children}
      </Show>
    </RuntimeSection>
  );
}
