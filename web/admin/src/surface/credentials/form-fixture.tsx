// 装配凭据导入表单所需的真实宿主上下文和重读世代。

import { createCell } from "@auction/web-shared/reactive/cell";
import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";
import type { AdminSessionHost } from "../../api/session";
import { AdminSessionHostContext } from "../auth/shell";
import {
  PendingCommandHostContext,
  PendingCommandsContext,
  usePendingCommandHost,
} from "../pending";
import { PendingCommandNotices } from "../pending/panel";
import { createCommandReload, usePendingCommands } from "../pending/sources";
import { CredentialCreateForm } from "./form";

interface FixtureProps {
  readonly host: AdminSessionHost;
  readonly navigate?: boolean;
}

/** 在显式传入的会话宿主内挂载真实导入表单。 */
export function CredentialFormFixture(props: FixtureProps): JSX.Element {
  const authenticated = createCell(true);
  return (
    <>
      <Show when={props.navigate === true}>
        <button type="button" onClick={() => authenticated.update((current) => !current)}>
          切换认证宿主
        </button>
      </Show>
      <Show when={authenticated()}>
        <AuthenticatedFixture {...props} />
      </Show>
    </>
  );
}

function AuthenticatedFixture(props: FixtureProps): JSX.Element {
  const pending = usePendingCommandHost(props.host);
  const visible = createCell(true);
  return (
    <AdminSessionHostContext value={props.host}>
      <PendingCommandHostContext value={pending}>
        <Show when={props.navigate === true}>
          <button type="button" onClick={() => visible.update((current) => !current)}>
            切换凭据页
          </button>
          <PendingCommandNotices host={pending} />
        </Show>
        <main id="admin-content" tabindex={-1}>
          <Show when={visible()}>
            <ImportSource />
          </Show>
        </main>
      </PendingCommandHostContext>
    </AdminSessionHostContext>
  );
}

function ImportSource(): JSX.Element {
  const reload = createCommandReload();
  const owner = usePendingCommands(reload, (entity) => entity.startsWith("credential:"));
  return (
    <PendingCommandsContext value={owner}>
      <CredentialCreateForm reloadGeneration={reload} />
      <output id="credential-fixture-reload">{reload().toString()}</output>
    </PendingCommandsContext>
  );
}
