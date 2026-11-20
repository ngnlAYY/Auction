// 在真实会话来源中挂载入站邮件关联的投递面板。

import { createCell } from "@auction/web-shared/reactive/cell";
import type { JSX } from "@solidjs/web";
import type { AdminMailIntakeDelivery } from "../../api/mail";
import { PendingCommandsContext } from "../pending";
import { createCommandReload, usePendingCommands } from "../pending/sources";
import type { AdminActionFeedback } from "../presentation";
import { MailDeliveriesPanel } from "./deliveries";

/** 关联投递行的真实处置入口。 */
export function LinkedDeliveryFixture(props: {
  readonly delivery: AdminMailIntakeDelivery;
}): JSX.Element {
  const reload = createCommandReload();
  const owner = usePendingCommands(reload, () => false);
  const notice = createCell<readonly [string, AdminActionFeedback] | null>(null);
  return (
    <PendingCommandsContext value={owner}>
      <MailDeliveriesPanel
        intakeId="00000000-0000-4000-8000-000000000022"
        deliveries={[props.delivery]}
        notice={notice}
        pageAlive={() => true}
        refresh={reload}
      />
    </PendingCommandsContext>
  );
}
