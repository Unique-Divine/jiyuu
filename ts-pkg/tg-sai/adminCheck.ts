import type { ChatMemberData } from "./adminAudit";
import { planAdminBootstrap } from "./adminAudit";
import { uniqueDivine } from "./cfg";
import { createMtcuteClient, fetchChatAuditInput } from "./mtcuteAdapter";

const DEFAULT_CHAT_ID = -5064008855;
const DEFAULT_TARGET_USER_ID = uniqueDivine.id;

interface Config {
  apiId: number;
  apiHash: string;
  session: string;
  chatId: number | string;
  targetUserId: number | string;
}

function requiredEnv(name: string): string {
  const value = process.env[name];
  if (!value) {
    throw new Error(`Missing required env var: ${name}`);
  }
  return value;
}

function parsePeer(value: string | undefined, fallback: number): number | string {
  if (!value) {
    return fallback;
  }
  const asNumber = Number(value);
  return Number.isFinite(asNumber) ? asNumber : value;
}

function getConfig(): Config {
  return {
    apiId: Number(requiredEnv("TELEGRAM_API_ID")),
    apiHash: requiredEnv("TELEGRAM_API_HASH"),
    session: requiredEnv("TELEGRAM_SESSION_STRING"),
    chatId: parsePeer(process.env.TELEGRAM_CHAT_ID, DEFAULT_CHAT_ID),
    targetUserId: parsePeer(
      process.env.TELEGRAM_TARGET_USER_ID,
      DEFAULT_TARGET_USER_ID,
    ),
  };
}

function showMember(label: string, member: ChatMemberData | null) {
  if (!member) {
    console.log(`${label}: not_present`);
    return;
  }

  console.log(`${label}:`);
  console.log(`  user_id: ${member.user.id}`);
  console.log(`  username: ${member.user.username ?? ""}`);
  console.log(`  status: ${member.status}`);
  console.log(`  is_admin: ${member.status === "admin"}`);
  console.log(`  is_creator: ${member.status === "creator"}`);
  console.log(`  has_admin_permissions: ${member.permissions !== null}`);
  if (member.permissions) {
    console.log(`  admin_permissions: ${JSON.stringify(member.permissions)}`);
  }
}

async function main() {
  const config = getConfig();
  const client = await createMtcuteClient({
    apiId: config.apiId,
    apiHash: config.apiHash,
    session: config.session,
  });

  const auditInput = await fetchChatAuditInput(client, {
    chatId: config.chatId,
    targetUserId: config.targetUserId,
  });
  const audit = planAdminBootstrap(auditInput);

  console.log("connection_ok");
  console.log(
    `session_user: ${auditInput.self.id} ${auditInput.self.username ?? ""}`,
  );
  console.log(`chat_id: ${auditInput.chatId}`);
  console.log(`target_user_id: ${auditInput.targetUserId}`);

  showMember("actor", auditInput.actor);
  showMember("target", auditInput.target);
  console.log(`actor_can_apply: ${audit.actorCanApply}`);
  console.log(
    `actor_can_transfer_ownership: ${audit.actorCanTransferOwnership}`,
  );
  console.log(
    `target_can_receive_ownership: ${audit.targetCanReceiveOwnership}`,
  );
  console.log(`proposed_actions: ${audit.proposedActions.join("; ")}`);

  await client.disconnect();
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
