import { describe, it, expect, beforeEach, afterEach, jest, mock, type Mock } from 'bun:test';
import { z } from 'zod';
import {
  CallToolRequestSchema,
  ListToolsRequestSchema,
} from '@modelcontextprotocol/sdk/types.js';
import { toolList } from './toolList.js';

// Bun has no automock and does not hoist module mocks. Register explicit
// mocks first, keep handles on them, then import the module under test.
// server.ts imports Client and MCPTransport only as types, so discord.js and
// transport.js need no mocks. Real zod and toolList replace their automocks.
const ServerMock = jest.fn();
mock.module('@modelcontextprotocol/sdk/server/index.js', () => ({
  Server: ServerMock
}));
// server.ts imports every handler by name and Bun checks named imports, so
// mock each export of the real module rather than a hand-kept subset.
const actualTools = await import('./tools/tools.js');
const toolsMock: Record<string, Mock<(...args: any[]) => any>> =
  Object.fromEntries(Object.keys(actualTools).map((name) => [name, jest.fn()]));
toolsMock.createToolContext = jest.fn(() => ({}));
mock.module('./tools/tools.js', () => toolsMock);
mock.module('./logger.js', () => ({
  info: jest.fn(),
  error: jest.fn(),
}));

const { DiscordMCPServer } = await import('./server.js');

const mockedServer = {
  setRequestHandler: jest.fn(),
};

const mockedTransport = {
  start: jest.fn(),
  stop: jest.fn(),
};

const mockedClient = {
  isReady: jest.fn(),
  user: null,
  token: null,
};

describe('DiscordMCPServer', () => {
  let discordMCPServer: InstanceType<typeof DiscordMCPServer>;
  let mockServer: any;
  let mockTransport: any;
  let mockClient: any;

  beforeEach(() => {
    // Mock setInterval globally before creating the instance
    jest.useFakeTimers();

    jest.clearAllMocks();

    // Setup mocks
    ServerMock.mockReturnValue(mockedServer);

    mockServer = mockedServer;
    mockTransport = mockedTransport;
    mockClient = mockedClient;

    discordMCPServer = new DiscordMCPServer(mockClient, mockTransport);
  });

  afterEach(() => {
    jest.useRealTimers();
  });

  describe('constructor', () => {
    it('should initialize server with correct parameters', () => {
      expect(ServerMock).toHaveBeenCalledWith(
        {
          name: 'MCP-Discord',
          version: '1.0.0'
        },
        {
          capabilities: {
            tools: {}
          }
        }
      );
      expect(mockServer.setRequestHandler).toHaveBeenCalledTimes(2); // ListTools and CallTool handlers
    });

    it('should setup request handlers', () => {
      expect(mockServer.setRequestHandler).toHaveBeenCalledWith(ListToolsRequestSchema, expect.any(Function));
      expect(mockServer.setRequestHandler).toHaveBeenCalledWith(CallToolRequestSchema, expect.any(Function));
    });
  });

  describe('request handlers', () => {
    let listToolsHandler: any;
    let callToolHandler: any;

    beforeEach(() => {
      // Extract the handlers that were registered
      const mockCalls = (mockServer.setRequestHandler as Mock<(...args: any[]) => any>).mock.calls;
      listToolsHandler = mockCalls.find((call: any[]) => call[0] === ListToolsRequestSchema)?.[1];
      callToolHandler = mockCalls.find((call: any[]) => call[0] === CallToolRequestSchema)?.[1];
    });

    describe('ListTools handler', () => {
      it('should return the tool list', async () => {
        const result = await listToolsHandler();
        
        expect(result).toEqual({
          tools: toolList
        });
      });
    });

    describe('CallTool handler', () => {
      it('should handle discord_create_category tool', async () => {
        const mockResult = { success: true };
        toolsMock.createCategoryHandler.mockResolvedValue(mockResult);

        const mockArgs = { categoryName: 'test-category' };
        const result = await callToolHandler({
          params: {
            name: 'discord_create_category',
            arguments: mockArgs
          }
        } as any);

        expect(toolsMock.createCategoryHandler).toHaveBeenCalledWith(
          mockArgs,
          expect.anything()
        );
        expect(result).toBe(mockResult);
      });

      it('should handle discord_edit_category tool', async () => {
        const mockResult = { success: true };
        toolsMock.editCategoryHandler.mockResolvedValue(mockResult);

        const result = await callToolHandler({
          params: {
            name: 'discord_edit_category',
            arguments: { categoryId: '123', newName: 'edited-category' }
          }
        } as any);

        expect(toolsMock.editCategoryHandler).toHaveBeenCalledWith(
          { categoryId: '123', newName: 'edited-category' },
          expect.anything()
        );
        expect(result).toBe(mockResult);
      });

      it('should handle discord_delete_category tool', async () => {
        const mockResult = { success: true };
        toolsMock.deleteCategoryHandler.mockResolvedValue(mockResult);

        const result = await callToolHandler({
          params: {
            name: 'discord_delete_category',
            arguments: { categoryId: '123' }
          }
        } as any);

        expect(toolsMock.deleteCategoryHandler).toHaveBeenCalledWith(
          { categoryId: '123' },
          expect.anything()
        );
        expect(result).toBe(mockResult);
      });

      it('should handle discord_login tool', async () => {
        const mockResult = { success: true };
        toolsMock.loginHandler.mockResolvedValue(mockResult);

        const result = await callToolHandler({
          params: {
            name: 'discord_login',
            arguments: { token: 'fake-token' }
          }
        } as any);

        expect(toolsMock.loginHandler).toHaveBeenCalledWith(
          { token: 'fake-token' },
          expect.anything()
        );
        expect(result).toBe(mockResult);
      });

      it('should handle discord_send tool', async () => {
        const mockResult = { success: true };
        toolsMock.sendMessageHandler.mockResolvedValue(mockResult);

        const result = await callToolHandler({
          params: {
            name: 'discord_send',
            arguments: { channelId: '123', message: 'hello' }
          }
        } as any);

        expect(toolsMock.sendMessageHandler).toHaveBeenCalledWith(
          { channelId: '123', message: 'hello' },
          expect.anything()
        );
        expect(result).toBe(mockResult);
      });

      it('should handle discord_get_forum_channels tool', async () => {
        const mockResult = { channels: [] };
        toolsMock.getForumChannelsHandler.mockResolvedValue(mockResult);

        const result = await callToolHandler({
          params: {
            name: 'discord_get_forum_channels',
            arguments: { serverId: '123' }
          }
        } as any);

        expect(toolsMock.getForumChannelsHandler).toHaveBeenCalledWith(
          { serverId: '123' },
          expect.anything()
        );
        expect(result).toBe(mockResult);
      });

      it('should handle discord_create_forum_post tool', async () => {
        const mockResult = { postId: '456' };
        toolsMock.createForumPostHandler.mockResolvedValue(mockResult);

        const result = await callToolHandler({
          params: {
            name: 'discord_create_forum_post',
            arguments: { channelId: '123', title: 'test', content: 'content' }
          }
        } as any);

        expect(toolsMock.createForumPostHandler).toHaveBeenCalledWith(
          { channelId: '123', title: 'test', content: 'content' },
          expect.anything()
        );
        expect(result).toBe(mockResult);
      });

      it('should handle discord_get_forum_post tool', async () => {
        const mockResult = { post: {} };
        toolsMock.getForumPostHandler.mockResolvedValue(mockResult);

        const result = await callToolHandler({
          params: {
            name: 'discord_get_forum_post',
            arguments: { postId: '456' }
          }
        } as any);

        expect(toolsMock.getForumPostHandler).toHaveBeenCalledWith(
          { postId: '456' },
          expect.anything()
        );
        expect(result).toBe(mockResult);
      });

      it('should handle discord_reply_to_forum tool', async () => {
        const mockResult = { replyId: '789' };
        toolsMock.replyToForumHandler.mockResolvedValue(mockResult);

        const result = await callToolHandler({
          params: {
            name: 'discord_reply_to_forum',
            arguments: { postId: '456', content: 'reply' }
          }
        } as any);

        expect(toolsMock.replyToForumHandler).toHaveBeenCalledWith(
          { postId: '456', content: 'reply' },
          expect.anything()
        );
        expect(result).toBe(mockResult);
      });

      it('should handle discord_delete_forum_post tool', async () => {
        const mockResult = { success: true };
        toolsMock.deleteForumPostHandler.mockResolvedValue(mockResult);

        const result = await callToolHandler({
          params: {
            name: 'discord_delete_forum_post',
            arguments: { postId: '456' }
          }
        } as any);

        expect(toolsMock.deleteForumPostHandler).toHaveBeenCalledWith(
          { postId: '456' },
          expect.anything()
        );
        expect(result).toBe(mockResult);
      });

      it('should handle discord_create_text_channel tool', async () => {
        const mockResult = { channelId: '999' };
        toolsMock.createTextChannelHandler.mockResolvedValue(mockResult);

        const result = await callToolHandler({
          params: {
            name: 'discord_create_text_channel',
            arguments: { serverId: '123', channelName: 'new-channel' }
          }
        } as any);

        expect(toolsMock.createTextChannelHandler).toHaveBeenCalledWith(
          { serverId: '123', channelName: 'new-channel' },
          expect.anything()
        );
        expect(result).toBe(mockResult);
      });

      it('should handle discord_delete_channel tool', async () => {
        const mockResult = { success: true };
        toolsMock.deleteChannelHandler.mockResolvedValue(mockResult);

        const result = await callToolHandler({
          params: {
            name: 'discord_delete_channel',
            arguments: { channelId: '999' }
          }
        } as any);

        expect(toolsMock.deleteChannelHandler).toHaveBeenCalledWith(
          { channelId: '999' },
          expect.anything()
        );
        expect(result).toBe(mockResult);
      });

      it('should handle discord_read_messages tool', async () => {
        const mockResult = { messages: [] };
        toolsMock.readMessagesHandler.mockResolvedValue(mockResult);

        const result = await callToolHandler({
          params: {
            name: 'discord_read_messages',
            arguments: { channelId: '123', limit: 10 }
          }
        } as any);

        expect(toolsMock.readMessagesHandler).toHaveBeenCalledWith(
          { channelId: '123', limit: 10 },
          expect.anything()
        );
        expect(result).toBe(mockResult);
      });

      it('should handle discord_get_server_info tool', async () => {
        const mockResult = { name: 'Test Server', id: '123' };
        toolsMock.getServerInfoHandler.mockResolvedValue(mockResult);

        const result = await callToolHandler({
          params: {
            name: 'discord_get_server_info',
            arguments: { serverId: '123' }
          }
        } as any);

        expect(toolsMock.getServerInfoHandler).toHaveBeenCalledWith(
          { serverId: '123' },
          expect.anything()
        );
        expect(result).toBe(mockResult);
      });

      it('should handle discord_add_reaction tool', async () => {
        const mockResult = { success: true };
        toolsMock.addReactionHandler.mockResolvedValue(mockResult);

        const result = await callToolHandler({
          params: {
            name: 'discord_add_reaction',
            arguments: { messageId: '456', emoji: '👍' }
          }
        } as any);

        expect(toolsMock.addReactionHandler).toHaveBeenCalledWith(
          { messageId: '456', emoji: '👍' },
          expect.anything()
        );
        expect(result).toBe(mockResult);
      });

      it('should handle discord_add_multiple_reactions tool', async () => {
        const mockResult = { success: true };
        toolsMock.addMultipleReactionsHandler.mockResolvedValue(mockResult);

        const result = await callToolHandler({
          params: {
            name: 'discord_add_multiple_reactions',
            arguments: { messageId: '456', emojis: ['👍', '👎'] }
          }
        } as any);

        expect(toolsMock.addMultipleReactionsHandler).toHaveBeenCalledWith(
          { messageId: '456', emojis: ['👍', '👎'] },
          expect.anything()
        );
        expect(result).toBe(mockResult);
      });

      it('should handle discord_remove_reaction tool', async () => {
        const mockResult = { success: true };
        toolsMock.removeReactionHandler.mockResolvedValue(mockResult);

        const result = await callToolHandler({
          params: {
            name: 'discord_remove_reaction',
            arguments: { messageId: '456', userId: '789', emoji: '👍' }
          }
        } as any);

        expect(toolsMock.removeReactionHandler).toHaveBeenCalledWith(
          { messageId: '456', userId: '789', emoji: '👍' },
          expect.anything()
        );
        expect(result).toBe(mockResult);
      });

      it('should handle discord_delete_message tool', async () => {
        const mockResult = { success: true };
        toolsMock.deleteMessageHandler.mockResolvedValue(mockResult);

        const result = await callToolHandler({
          params: {
            name: 'discord_delete_message',
            arguments: { messageId: '456', channelId: '123' }
          }
        } as any);

        expect(toolsMock.deleteMessageHandler).toHaveBeenCalledWith(
          { messageId: '456', channelId: '123' },
          expect.anything()
        );
        expect(result).toBe(mockResult);
      });

      it('should handle discord_create_webhook tool', async () => {
        const mockResult = { webhookId: 'abc' };
        toolsMock.createWebhookHandler.mockResolvedValue(mockResult);

        const result = await callToolHandler({
          params: {
            name: 'discord_create_webhook',
            arguments: { channelId: '123', name: 'webhook-name' }
          }
        } as any);

        expect(toolsMock.createWebhookHandler).toHaveBeenCalledWith(
          { channelId: '123', name: 'webhook-name' },
          expect.anything()
        );
        expect(result).toBe(mockResult);
      });

      it('should handle discord_send_webhook_message tool', async () => {
        const mockResult = { messageId: 'def' };
        toolsMock.sendWebhookMessageHandler.mockResolvedValue(mockResult);

        const result = await callToolHandler({
          params: {
            name: 'discord_send_webhook_message',
            arguments: { webhookId: 'abc', message: 'hello' }
          }
        } as any);

        expect(toolsMock.sendWebhookMessageHandler).toHaveBeenCalledWith(
          { webhookId: 'abc', message: 'hello' },
          expect.anything()
        );
        expect(result).toBe(mockResult);
      });

      it('should handle discord_edit_webhook tool', async () => {
        const mockResult = { success: true };
        toolsMock.editWebhookHandler.mockResolvedValue(mockResult);

        const result = await callToolHandler({
          params: {
            name: 'discord_edit_webhook',
            arguments: { webhookId: 'abc', newName: 'updated-name' }
          }
        } as any);

        expect(toolsMock.editWebhookHandler).toHaveBeenCalledWith(
          { webhookId: 'abc', newName: 'updated-name' },
          expect.anything()
        );
        expect(result).toBe(mockResult);
      });

      it('should handle discord_delete_webhook tool', async () => {
        const mockResult = { success: true };
        toolsMock.deleteWebhookHandler.mockResolvedValue(mockResult);

        const result = await callToolHandler({
          params: {
            name: 'discord_delete_webhook',
            arguments: { webhookId: 'abc' }
          }
        } as any);

        expect(toolsMock.deleteWebhookHandler).toHaveBeenCalledWith(
          { webhookId: 'abc' },
          expect.anything()
        );
        expect(result).toBe(mockResult);
      });

      it('should handle discord_list_servers tool', async () => {
        const mockResult = { servers: [] };
        toolsMock.listServersHandler.mockResolvedValue(mockResult);

        const result = await callToolHandler({
          params: {
            name: 'discord_list_servers',
            arguments: {}
          }
        } as any);

        expect(toolsMock.listServersHandler).toHaveBeenCalledWith(
          {},
          expect.anything()
        );
        expect(result).toBe(mockResult);
      });

      it('should handle discord_search_messages tool', async () => {
        const mockResult = { messages: [] };
        toolsMock.searchMessagesHandler.mockResolvedValue(mockResult);

        const result = await callToolHandler({
          params: {
            name: 'discord_search_messages',
            arguments: { query: 'test', channelId: '123' }
          }
        } as any);

        expect(toolsMock.searchMessagesHandler).toHaveBeenCalledWith(
          { query: 'test', channelId: '123' },
          expect.anything()
        );
        expect(result).toBe(mockResult);
      });

      it('should return error for unknown tool', async () => {
        const result = await callToolHandler({
          params: {
            name: 'unknown_tool',
            arguments: {}
          }
        } as any);

        expect(result).toEqual({
          content: [{ type: "text", text: "Error executing tool: Unknown tool: unknown_tool" }],
          isError: true
        });
      });

      it('should handle Zod validation errors', async () => {
        const mockZodError = new z.ZodError([
          { code: 'custom', path: ['param'], message: 'Invalid param' }
        ]);

        toolsMock.sendMessageHandler.mockRejectedValue(mockZodError);

        const result = await callToolHandler({
          params: {
            name: 'discord_send',
            arguments: {}
          }
        } as any);

        expect(result).toEqual({
          content: [{ type: "text", text: "Invalid arguments: param: Invalid param" }],
          isError: true
        });
      });

      it('should handle generic errors', async () => {
        const mockError = new Error('Generic error');

        toolsMock.sendMessageHandler.mockRejectedValue(mockError);

        const result = await callToolHandler({
          params: {
            name: 'discord_send',
            arguments: {}
          }
        } as any);

        expect(result).toEqual({
          content: [{ type: "text", text: "Error executing tool: Generic error" }],
          isError: true
        });
      });
    });
  });

  describe('start', () => {
    it('should start the transport and set up client state logging', async () => {
      jest.useFakeTimers();

      // Mock setInterval since it's a global function
      const setIntervalSpy = jest.spyOn(global, 'setInterval');

      await discordMCPServer.start();

      expect(mockTransport.start).toHaveBeenCalledWith(mockServer);
      expect(setIntervalSpy).toHaveBeenCalledWith(expect.any(Function), 10000);

      // Clean up timer
      jest.useRealTimers();
    });

    it('should add client to server context', async () => {
      await discordMCPServer.start();

      // Check that client was added to the server context
      expect((mockServer as any)._context).toBeDefined();
      expect((mockServer as any).client).toBeDefined();
    });
  });

  describe('stop', () => {
    it('should stop the transport and clear the interval', async () => {
      jest.useFakeTimers();
      // First start to initialize the interval
      await discordMCPServer.start();

      const clearIntervalSpy = jest.spyOn(global, 'clearInterval');
      await discordMCPServer.stop();

      expect(clearIntervalSpy).toHaveBeenCalled();
      expect(mockTransport.stop).toHaveBeenCalled();
      jest.useRealTimers();
    });

    it('should stop even if interval is not set', async () => {
      // Don't call start, so no interval is set
      await discordMCPServer.stop();

      expect(mockTransport.stop).toHaveBeenCalled();
    });
  });
});