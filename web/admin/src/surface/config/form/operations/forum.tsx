// 论坛同步参数编辑控件。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";

import type { RuntimeConfigForm } from "../../model";
import { NumberField, SwitchField } from "..";
import { ManagedSection } from ".";

/** 论坛同步分组。 */
export function ForumFields(props: {
  readonly form: Cell<RuntimeConfigForm>;
  readonly disabled: boolean;
  readonly onDirty: () => void;
}): JSX.Element {
  return (
    <ManagedSection
      form={props.form}
      disabled={props.disabled}
      path="operations.forum.managed"
      title={t("admin-runtime-op-section-forum")}
      note={t("admin-runtime-op-note-forum")}
      onDirty={props.onDirty}
    >
      <div class="admin-runtime-field-grid">
        <SwitchField
          form={props.form}
          disabled={props.disabled}
          path="operations.forum.enabled"
          label={t("admin-runtime-op-enabled")}
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.forum.batch_size"
          label={t("admin-runtime-op-batch-size")}
          minimum="1"
          maximum="100"
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.forum.poll_interval_seconds"
          label={t("admin-runtime-op-poll-interval-seconds")}
          minimum="1"
          maximum="86400"
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.forum.jitter_seconds"
          label={t("admin-runtime-op-jitter-seconds")}
          minimum="0"
          maximum="86400"
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.forum.maximum_response_bytes"
          label={t("admin-runtime-op-maximum-response-bytes")}
          minimum="1"
          maximum="8388608"
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.forum.maximum_topic_pages"
          label={t("admin-runtime-op-maximum-topic-pages")}
          minimum="1"
          maximum="20"
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.forum.minimum_request_interval_millis"
          label={t("admin-runtime-op-minimum-request-interval-millis")}
          minimum="1"
          maximum="60000"
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.forum.request_timeout_seconds"
          label={t("admin-runtime-op-request-timeout-seconds")}
          minimum="1"
          maximum="300"
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.forum.item_timeout_seconds"
          label={t("admin-runtime-op-item-timeout-seconds")}
          minimum="1"
          maximum="3600"
          onDirty={props.onDirty}
        />
        <NumberField
          form={props.form}
          disabled={props.disabled}
          path="operations.forum.lease_duration_seconds"
          label={t("admin-runtime-op-lease-duration-seconds")}
          minimum="1"
          maximum="86400"
          onDirty={props.onDirty}
        />
      </div>
    </ManagedSection>
  );
}
