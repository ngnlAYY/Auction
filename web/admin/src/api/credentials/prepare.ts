// 凭据写命令的准备：在首次提交时冻结输入并生成一次正文。

import type {
  AdminCredentialCreateRequest,
  AdminCredentialDeleteRequest,
  AdminCredentialMutationRequest,
} from "@auction/contracts/dto/admin/credentials";
import { buildAdminCredentialCreateRequest } from "@auction/contracts/dto/admin/credentials";
import { err, ok, type Result } from "@auction/web-shared/result";

import { type AdminWriteNotSent, invalidRequestError, writeNotSent } from "../failure";
import { type PreparedJsonCommand, prepareJsonCommand } from "../transport";
import { type AdminCredential, parseRustUuid } from "./decode";

/** 导入命令的冻结正文与安全回执核对字段。 */
export interface PreparedAdminCredentialCreate {
  readonly command_id: string;
  readonly kind: string;
  readonly world: string;
  readonly label: string;
  readonly player_name: string | null;
  readonly prepared: PreparedJsonCommand;
}

/** 修改命令的冻结正文与回执核对字段；同一对象同时供首次发送与重试使用。 */
export interface PreparedAdminCredentialMutation {
  readonly command_id: string;
  readonly previous: AdminCredential;
  readonly credential_id: string;
  readonly action: string;
  readonly label: string | null;
  readonly status: string | null;
  readonly prepared: PreparedJsonCommand;
}

/** 删除命令的冻结正文与回执核对字段。 */
export interface PreparedAdminCredentialDelete {
  readonly command_id: string;
  readonly previous: AdminCredential;
  readonly credential_id: string;
  readonly prepared: PreparedJsonCommand;
}

/** 逐字段复制安全投影，保证 await 之后不再读取调用方可能变化的草稿对象。 */
export function copyCredential(value: AdminCredential): AdminCredential {
  return {
    id: value.id,
    kind: value.kind,
    world: value.world,
    label: value.label,
    player_name: value.player_name,
    status: value.status,
    state_version: value.state_version,
    created_at: value.created_at,
    rotated_at: value.rotated_at,
    updated_at: value.updated_at,
  };
}

/** 生成导入正文与安全回执核对字段；Cookie 和论坛密码只留在正文内。 */
export function prepareAdminCredentialCreate(
  command: AdminCredentialCreateRequest,
): Result<PreparedAdminCredentialCreate, AdminWriteNotSent> {
  let commandId: string;
  let kind: string;
  let world: string;
  let label: string;
  let playerName: string | undefined;
  let secret: string | undefined;
  let login: { username: string; password: string } | undefined;
  try {
    commandId = command.command_id;
    kind = command.kind;
    world = command.world;
    label = command.label;
    playerName = command.player_name;
    secret = command.secret;
    const rawLogin = command.login;
    login =
      rawLogin === undefined
        ? undefined
        : { username: rawLogin.username, password: rawLogin.password };
  } catch (error) {
    return err(writeNotSent(invalidRequestError(error)));
  }
  const canonicalCommandId = parseRustUuid(commandId);
  if (canonicalCommandId === null) {
    return err(writeNotSent("invalid_request"));
  }
  const request = buildAdminCredentialCreateRequest({
    command_id: canonicalCommandId,
    kind,
    world,
    label,
    ...(playerName === undefined ? {} : { player_name: playerName }),
    ...(secret === undefined ? {} : { secret }),
    ...(login === undefined ? {} : { login }),
  });
  const prepared = prepareJsonCommand(request);
  if (!prepared.ok) {
    return err(prepared.error);
  }
  return ok(
    Object.freeze({
      command_id: canonicalCommandId,
      kind,
      world,
      label,
      player_name: playerName ?? null,
      prepared: prepared.value,
    }),
  );
}

/** 冻结导入输入，首次执行时生成正文，之后的手动重试复用同一份正文。 */
export function prepareAdminCredentialCreateOnce(
  input: Omit<AdminCredentialCreateRequest, "command_id">,
): (commandId: string) => Result<PreparedAdminCredentialCreate, AdminWriteNotSent> {
  const login = input.login;
  let state:
    | { kind: "input"; request: typeof input }
    | { kind: "prepared"; result: Result<PreparedAdminCredentialCreate, AdminWriteNotSent> } = {
    kind: "input",
    request: { ...input, ...(login === undefined ? {} : { login: { ...login } }) },
  };
  return (commandId) => {
    if (state.kind === "input") {
      const result = prepareAdminCredentialCreate({ ...state.request, command_id: commandId });
      // 序列化后释放冻结输入，宿主只保留一份可重试的正文。
      state = { kind: "prepared", result };
    }
    return state.result;
  };
}

/** 在首次提交时冻结被修改凭据与操作参数；正文只生成一次。 */
export function prepareAdminCredentialMutation(
  commandId: string,
  credential: AdminCredential,
  action: string,
  label: string | null,
  status: string | null,
): Result<PreparedAdminCredentialMutation, AdminWriteNotSent> {
  const canonicalCommandId = parseRustUuid(commandId);
  if (canonicalCommandId === null) {
    return err(writeNotSent("invalid_request"));
  }
  let previous: AdminCredential;
  try {
    previous = copyCredential(credential);
  } catch (error) {
    return err(writeNotSent(invalidRequestError(error)));
  }
  const credentialId = parseRustUuid(previous.id);
  if (credentialId === null || !Number.isSafeInteger(previous.state_version)) {
    return err(writeNotSent("invalid_request"));
  }
  const request: AdminCredentialMutationRequest = {
    command_id: canonicalCommandId,
    credential_id: credentialId,
    expected_state_version: previous.state_version,
    action,
    label,
    status,
  };
  const prepared = prepareJsonCommand(request);
  if (!prepared.ok) {
    return err(prepared.error);
  }
  return ok(
    Object.freeze({
      command_id: canonicalCommandId,
      previous: Object.freeze(previous),
      credential_id: credentialId,
      action,
      label,
      status,
      prepared: prepared.value,
    }),
  );
}

/** 在首次提交时冻结被删除凭据；正文只生成一次。 */
export function prepareAdminCredentialDelete(
  commandId: string,
  credential: AdminCredential,
): Result<PreparedAdminCredentialDelete, AdminWriteNotSent> {
  const canonicalCommandId = parseRustUuid(commandId);
  if (canonicalCommandId === null) {
    return err(writeNotSent("invalid_request"));
  }
  let previous: AdminCredential;
  try {
    previous = copyCredential(credential);
  } catch (error) {
    return err(writeNotSent(invalidRequestError(error)));
  }
  const credentialId = parseRustUuid(previous.id);
  if (credentialId === null || !Number.isSafeInteger(previous.state_version)) {
    return err(writeNotSent("invalid_request"));
  }
  const request: AdminCredentialDeleteRequest = {
    command_id: canonicalCommandId,
    credential_id: credentialId,
    expected_state_version: previous.state_version,
  };
  const prepared = prepareJsonCommand(request);
  if (!prepared.ok) {
    return err(prepared.error);
  }
  return ok(
    Object.freeze({
      command_id: canonicalCommandId,
      previous: Object.freeze(previous),
      credential_id: credentialId,
      prepared: prepared.value,
    }),
  );
}
