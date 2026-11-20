// 运行配置读取的加载、冲突与错误提示。

import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Match, Switch } from "solid-js";

export interface ReadFeedback {
  readonly kind: "loading" | "error" | "conflict";
  readonly title: string;
}

export function ReadFeedbackPanel(props: {
  readonly feedback: ReadFeedback | null;
  readonly onRetry: () => void;
}): JSX.Element {
  return (
    <Switch>
      <Match when={props.feedback?.kind === "conflict"}>
        <p class="admin-panel__note action-feedback action-feedback--error" role="alert">
          {props.feedback?.title}
        </p>
      </Match>
      <Match when={props.feedback}>
        {(feedback) => (
          <AsyncState
            kind={feedback().kind === "error" ? "error" : "loading"}
            title={feedback().title}
            detail=""
            {...(feedback().kind === "error"
              ? { retryLabel: t("admin-retry"), onRetry: props.onRetry }
              : {})}
          />
        )}
      </Match>
    </Switch>
  );
}
