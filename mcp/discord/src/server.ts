import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { Status, type Client } from "discord.js";
import { z } from "zod";
import {
  CallToolRequestSchema,
  ListToolsRequestSchema,
  type CallToolRequest,
} from "@modelcontextprotocol/sdk/types.js";
import { fileURLToPath } from "node:url";
import { toolList } from './toolList.js';
import {
  createToolContext,
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
} from './tools/tools.js';
import { MCPTransport } from './transport.js';
import { info, error } from './logger.js';
import {
  JiyuuMcpInfo,
  MCP_INFO_INSTRUCTIONS,
  MCP_INFO_TOOL,
  withUpdateNotice,
} from './jiyuuMcpInfo.js';

const CLIENT_STATE_LOG_INTERVAL_MS = 5 * 60 * 1000; // was 10_000

export function isDiscordReady(client: Client): boolean {
  // The manager can remain Ready while a gateway shard reconnects.
  return client.isReady() && client.ws.shards.size > 0
    && client.ws.shards.every(shard => shard.status === Status.Ready);
}

export class DiscordMCPServer {
  private toolContext: ReturnType<typeof createToolContext>;
  private clientStatusInterval: NodeJS.Timeout | null = null;

  constructor(
    private client: Client, 
    private transport: MCPTransport,
    private mcpInfo: JiyuuMcpInfo = new JiyuuMcpInfo(
      "discord",
      fileURLToPath(new URL("..", import.meta.url)),
      ["DISCORD_"],
    ),
    private shared = false,
  ) {
    this.toolContext = createToolContext(client);
  }

  private createServer(): Server {
    const server = new Server(
      {
        name: "MCP-Discord",
        version: this.mcpInfo.identity.version
      },
      {
        capabilities: {
          tools: {}
        },
        instructions: MCP_INFO_INSTRUCTIONS
      }
    );

    // Set up the tool list
    server.setRequestHandler(ListToolsRequestSchema, async () => {
      return {
        tools: [...toolList.filter(tool => !this.shared || tool.name !== 'discord_login'), MCP_INFO_TOOL]
      };
    });

    // Handle tool execution requests. The first successful result after the
    // background update check finds a newer release carries a one-line notice.
    server.setRequestHandler(CallToolRequestSchema, async (request) => {
      const result = request.params.name === MCP_INFO_TOOL.name
        ? await this.mcpInfo.callTool()
        : await this.callTool(request);
      return withUpdateNotice(result, this.mcpInfo.notifier);
    });
    return server;
  }

  private async callTool(request: CallToolRequest): Promise<any> {
      const { name, arguments: args } = request.params;

      if (this.shared && name === 'discord_login') {
        return { content: [{ type: 'text', text: 'Discord login is configured by the shared service' }], isError: true };
      }
      if (this.shared && !isDiscordReady(this.client)) {
        return { content: [{ type: 'text', text: 'Discord is not ready' }], isError: true };
      }

      try {
        let toolResponse;
        switch (name) {
          case "discord_create_category":
            toolResponse = await createCategoryHandler(args, this.toolContext);
            return toolResponse;
          case "discord_edit_category":
            toolResponse = await editCategoryHandler(args, this.toolContext);
            return toolResponse;
          case "discord_delete_category":
            toolResponse = await deleteCategoryHandler(args, this.toolContext);
            return toolResponse;

          case "discord_login": toolResponse = await loginHandler(args, this.toolContext);
            this.logClientState("after discord_login handler");
            return toolResponse;

          case "discord_send":
            this.logClientState("before discord_send handler");
            toolResponse = await sendMessageHandler(args, this.toolContext);
            return toolResponse;

          case "discord_get_forum_channels":
            this.logClientState("before discord_get_forum_channels handler");
            toolResponse = await getForumChannelsHandler(args, this.toolContext);
            return toolResponse;

          case "discord_create_forum_post":
            this.logClientState("before discord_create_forum_post handler");
            toolResponse = await createForumPostHandler(args, this.toolContext);
            return toolResponse;

          case "discord_get_forum_post":
            this.logClientState("before discord_get_forum_post handler");
            toolResponse = await getForumPostHandler(args, this.toolContext);
            return toolResponse;

          case "discord_list_forum_threads":
            this.logClientState("before discord_list_forum_threads handler");
            toolResponse = await listForumThreadsHandler(args, this.toolContext);
            return toolResponse;

          case "discord_reply_to_forum":
            this.logClientState("before discord_reply_to_forum handler");
            toolResponse = await replyToForumHandler(args, this.toolContext);
            return toolResponse;

          case "discord_delete_forum_post":
            this.logClientState("before discord_delete_forum_post handler");
            toolResponse = await deleteForumPostHandler(args, this.toolContext);
            return toolResponse;

          case "discord_get_forum_tags":
            this.logClientState("before discord_get_forum_tags handler");
            toolResponse = await getForumTagsHandler(args, this.toolContext);
            return toolResponse;

          case "discord_set_forum_tags":
            this.logClientState("before discord_set_forum_tags handler");
            toolResponse = await setForumTagsHandler(args, this.toolContext);
            return toolResponse;

          case "discord_update_forum_post":
            this.logClientState("before discord_update_forum_post handler");
            toolResponse = await updateForumPostHandler(args, this.toolContext);
            return toolResponse;

          case "discord_edit_message":
            this.logClientState("before discord_edit_message handler");
            toolResponse = await editMessageHandler(args, this.toolContext);
            return toolResponse;

          case "discord_create_text_channel":
            this.logClientState("before discord_create_text_channel handler");
            toolResponse = await createTextChannelHandler(args, this.toolContext);
            return toolResponse;

          case "discord_create_forum_channel":
            this.logClientState("before discord_create_forum_channel handler");
            toolResponse = await createForumChannelHandler(args, this.toolContext);
            return toolResponse;

          case "discord_edit_channel":
            this.logClientState("before discord_edit_channel handler");
            toolResponse = await editChannelHandler(args, this.toolContext);
            return toolResponse;

          case "discord_delete_channel":
            this.logClientState("before discord_delete_channel handler");
            toolResponse = await deleteChannelHandler(args, this.toolContext);
            return toolResponse;

          case "discord_read_messages":
            this.logClientState("before discord_read_messages handler");
            toolResponse = await readMessagesHandler(args, this.toolContext);
            return toolResponse;

          case "discord_get_server_info":
            this.logClientState("before discord_get_server_info handler");
            toolResponse = await getServerInfoHandler(args, this.toolContext);
            return toolResponse;

          case "discord_add_reaction":
            this.logClientState("before discord_add_reaction handler");
            toolResponse = await addReactionHandler(args, this.toolContext);
            return toolResponse;

          case "discord_add_multiple_reactions":
            this.logClientState("before discord_add_multiple_reactions handler");
            toolResponse = await addMultipleReactionsHandler(args, this.toolContext);
            return toolResponse;

          case "discord_remove_reaction":
            this.logClientState("before discord_remove_reaction handler");
            toolResponse = await removeReactionHandler(args, this.toolContext);
            return toolResponse;

          case "discord_get_reaction_users":
            this.logClientState("before discord_get_reaction_users handler");
            toolResponse = await getReactionUsersHandler(args, this.toolContext);
            return toolResponse;

          case "discord_delete_message":
            this.logClientState("before discord_delete_message handler");
            toolResponse = await deleteMessageHandler(args, this.toolContext);
            return toolResponse;

          case "discord_create_webhook":
            this.logClientState("before discord_create_webhook handler");
            toolResponse = await createWebhookHandler(args, this.toolContext);
            return toolResponse;

          case "discord_send_webhook_message":
            this.logClientState("before discord_send_webhook_message handler");
            toolResponse = await sendWebhookMessageHandler(args, this.toolContext);
            return toolResponse;

          case "discord_edit_webhook":
            this.logClientState("before discord_edit_webhook handler");
            toolResponse = await editWebhookHandler(args, this.toolContext);
            return toolResponse;

          case "discord_delete_webhook":
            this.logClientState("before discord_delete_webhook handler");
            toolResponse = await deleteWebhookHandler(args, this.toolContext);
            return toolResponse;

          case "discord_list_servers":
            this.logClientState("before discord_list_servers handler");
            toolResponse = await listServersHandler(args, this.toolContext);
            return toolResponse;

          case "discord_search_messages":
            this.logClientState("before discord_search_messages handler");
            toolResponse = await searchMessagesHandler(args, this.toolContext);
            return toolResponse;

          case "discord_list_roles":
            toolResponse = await listRolesHandler(args, this.toolContext);
            return toolResponse;

          case "discord_create_role":
            toolResponse = await createRoleHandler(args, this.toolContext);
            return toolResponse;

          case "discord_edit_role":
            toolResponse = await editRoleHandler(args, this.toolContext);
            return toolResponse;

          case "discord_delete_role":
            toolResponse = await deleteRoleHandler(args, this.toolContext);
            return toolResponse;

          case "discord_assign_role":
            toolResponse = await assignRoleHandler(args, this.toolContext);
            return toolResponse;

          case "discord_remove_role":
            toolResponse = await removeRoleHandler(args, this.toolContext);
            return toolResponse;

          case "discord_list_members":
            toolResponse = await listMembersHandler(args, this.toolContext);
            return toolResponse;

          case "discord_get_member":
            toolResponse = await getMemberHandler(args, this.toolContext);
            return toolResponse;

          case "discord_create_voice_channel":
            toolResponse = await createVoiceChannelHandler(args, this.toolContext);
            return toolResponse;

          case "discord_set_channel_permissions":
            toolResponse = await setChannelPermissionsHandler(args, this.toolContext);
            return toolResponse;

          case "discord_remove_channel_permissions":
            toolResponse = await removeChannelPermissionsHandler(args, this.toolContext);
            return toolResponse;

          default:
            throw new Error(`Unknown tool: ${name}`);
        }
      } catch (error) {
        if (error instanceof z.ZodError) {
          return {
            content: [{
              type: "text",
              text: `Invalid arguments: ${error.errors
                .map((e: z.ZodIssue) => `${e.path.join(".")}: ${e.message}`)
                .join(", ")}`
            }],
            isError: true
          };
        }

        const errorMessage = error instanceof Error ? error.message : 'Unknown error';
        return {
          content: [{ type: "text", text: `Error executing tool: ${errorMessage}` }],
          isError: true
        };
      }
  }

  private logClientState(context: string) {
    info(`Discord client state [${context}]: ${JSON.stringify({
      isReady: this.client.isReady(),
      hasToken: !!this.client.token,
      user: this.client.user ? {
        id: this.client.user.id,
        tag: this.client.user.tag,
      } : null
    })}`);
  }

  async start() {
    this.mcpInfo.notifier.start();

    // Setup periodic client state logging
    this.clientStatusInterval = setInterval(() => {
      this.logClientState("periodic check");
    }, CLIENT_STATE_LOG_INTERVAL_MS);
    
    await this.transport.start(() => this.createServer());
  }

  async stop() {
    // Clear the periodic check interval
    if (this.clientStatusInterval) {
      clearInterval(this.clientStatusInterval);
      this.clientStatusInterval = null;
    }

    try {
      await this.transport.stop();
    } finally {
      try {
        await this.client.destroy();
      } catch {
        error('Could not destroy the Discord client');
      }
    }
  }
}
