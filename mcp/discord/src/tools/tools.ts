import { Client } from "discord.js";
import { z } from "zod";
import { ToolResponse, ToolContext, ToolHandler } from "./types.js";
import { loginHandler } from './login.js';
import { sendMessageHandler, editMessageHandler } from './send-message.js';
import {
  getForumChannelsHandler,
  createForumPostHandler,
  getForumPostHandler,
  listForumThreadsHandler,
  replyToForumHandler,
  deleteForumPostHandler,
  getForumTagsHandler,
  setForumTagsHandler,
  updateForumPostHandler
} from './forum.js';
import {
  createTextChannelHandler,
  createForumChannelHandler,
  editChannelHandler,
  deleteChannelHandler,
  readMessagesHandler,
  createCategoryHandler,
  editCategoryHandler,
  deleteCategoryHandler,
  createVoiceChannelHandler,
  setChannelPermissionsHandler,
  removeChannelPermissionsHandler
} from './channel.js';
import {
  getServerInfoHandler,
  listServersHandler,
  searchMessagesHandler
} from "./server.js";
import {
  addReactionHandler,
  addMultipleReactionsHandler,
  removeReactionHandler,
  getReactionUsersHandler,
  deleteMessageHandler
} from './reactions.js';
import {
  createWebhookHandler,
  sendWebhookMessageHandler,
  editWebhookHandler,
  deleteWebhookHandler
} from './webhooks.js';
import {
  listRolesHandler,
  createRoleHandler,
  editRoleHandler,
  deleteRoleHandler,
  assignRoleHandler,
  removeRoleHandler,
  listMembersHandler,
  getMemberHandler
} from './roles.js';

// Export tool handlers
export {
  loginHandler,
  sendMessageHandler,
  getForumChannelsHandler,
  createForumPostHandler,
  getForumPostHandler,
  listForumThreadsHandler,
  replyToForumHandler,
  deleteForumPostHandler,
  getForumTagsHandler,
  setForumTagsHandler,
  updateForumPostHandler,
  editMessageHandler,
  createTextChannelHandler,
  createForumChannelHandler,
  editChannelHandler,
  deleteChannelHandler,
  readMessagesHandler,
  getServerInfoHandler,
  addReactionHandler,
  addMultipleReactionsHandler,
  removeReactionHandler,
  getReactionUsersHandler,
  deleteMessageHandler,
  createWebhookHandler,
  sendWebhookMessageHandler,
  editWebhookHandler,
  deleteWebhookHandler,
  createCategoryHandler,
  editCategoryHandler,
  deleteCategoryHandler,
  createVoiceChannelHandler,
  setChannelPermissionsHandler,
  removeChannelPermissionsHandler,
  listRolesHandler,
  createRoleHandler,
  editRoleHandler,
  deleteRoleHandler,
  assignRoleHandler,
  removeRoleHandler,
  listMembersHandler,
  getMemberHandler,
  listServersHandler,
  searchMessagesHandler
};

// Jiyuu: keep this `export type`. Bun transpiles each file on its own and
// cannot tell these are interfaces, so a plain `export { ... }` fails at
// startup with "export 'ToolContext' not found". tsc erased it upstream.
export type { ToolResponse, ToolContext, ToolHandler };

// Create tool context
export function createToolContext(client: Client): ToolContext {
  return { client };
}
