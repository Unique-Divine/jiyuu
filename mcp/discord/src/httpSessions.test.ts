import { afterEach, beforeEach, describe, expect, it } from 'bun:test';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';
import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { z } from 'zod';
import { StreamableHttpTransport } from './transport.js';
import { freePort, postInitialize } from './test-support/httpEntrypoint.js';

describe('HTTP sessions', () => {
    const idleTimeoutMs = 180;
    let port: number;
    let transport: StreamableHttpTransport;
    let clients: Client[];
    let ready: boolean;
    let entered: ReturnType<typeof Promise.withResolvers<void>>;
    let release: ReturnType<typeof Promise.withResolvers<void>>;

    beforeEach(async () => {
        port = freePort();
        clients = [];
        ready = false;
        entered = Promise.withResolvers<void>();
        release = Promise.withResolvers<void>();
        transport = new StreamableHttpTransport({ port, idleTimeoutMs, isReady: () => ready });
        await transport.start(() => {
            const server = new McpServer({ name: 'http-session-fixture', version: '1' });
            server.tool('echo', { message: z.string() }, async ({ message }) => {
                if (message === 'delayed') {
                    entered.resolve();
                    await release.promise;
                }
                return { content: [{ type: 'text', text: message }] };
            });
            return server.server;
        });
    });

    afterEach(async () => {
        release.resolve();
        await Promise.all(clients.map(client => client.close()));
        await transport.stop();
    });

    async function connect() {
        const client = new Client({ name: 'fixture client', version: '1' });
        clients.push(client);
        const toolIds: unknown[] = [];
        const connection = new StreamableHTTPClientTransport(new URL(`http://127.0.0.1:${port}/mcp`), {
            fetch: async (input, init) => {
                if (typeof init?.body === 'string') {
                    const request = JSON.parse(init.body);
                    if (request.method === 'tools/call') toolIds.push(request.id);
                }
                return fetch(input, init);
            }
        });
        await client.connect(connection);
        return { client, connection, toolIds };
    }

    function echo(client: Client, message: string) {
        return client.callTool({ name: 'echo', arguments: { message } }, undefined, { timeout: 1_500 });
    }

    function post(sessionId: string) {
        return fetch(`http://127.0.0.1:${port}/mcp`, {
            method: 'POST',
            headers: {
                'Mcp-Session-Id': sessionId,
                'content-type': 'application/json',
                accept: 'application/json, text/event-stream',
            },
            body: JSON.stringify({ jsonrpc: '2.0', id: 4, method: 'tools/list' })
        });
    }

    it('routes overlapping calls with the same client-generated ID to their own responses', async () => {
        const { client: first, connection: firstConnection, toolIds: firstIds } = await connect();
        const { client: second, connection: secondConnection, toolIds: secondIds } = await connect();
        expect(firstConnection.sessionId).toBeString();
        expect(secondConnection.sessionId).not.toBe(firstConnection.sessionId);

        // Both SDK clients start at the same initial request ID and these
        // are the first tool calls on each connection. Keep them in flight together.
        const delayed = echo(first, 'delayed');
        await entered.promise;
        expect((await echo(second, 'immediate')).content).toEqual([{ type: 'text', text: 'immediate' }]);
        release.resolve();
        expect((await delayed).content).toEqual([{ type: 'text', text: 'delayed' }]);
        expect(firstIds).toHaveLength(1);
        expect(secondIds).toEqual(firstIds);
    });

    it('closes one session explicitly without disrupting another', async () => {
        const { connection } = await connect();
        const { client } = await connect();
        const oldId = connection.sessionId!;
        await connection.terminateSession();

        const expired = await post(oldId);
        expect(expired.status).toBe(404);
        await expired.body?.cancel();
        expect((await echo(client, 'still connected')).content).toEqual([{ type: 'text', text: 'still connected' }]);
    });

    it('expires an abandoned initialization while an in-flight call can finish', async () => {
        const abandoned = await postInitialize(port);
        const abandonedId = abandoned.headers.get('Mcp-Session-Id')!;
        expect(abandonedId).toBeString();
        await abandoned.text();

        const { client } = await connect();
        const delayed = echo(client, 'delayed');
        await entered.promise;
        await Bun.sleep(idleTimeoutMs * 3);

        const expired = await post(abandonedId);
        expect(expired.status).toBe(404);
        await expired.body?.cancel();
        release.resolve();
        expect((await delayed).content).toEqual([{ type: 'text', text: 'delayed' }]);
        expect((await echo(client, 'still connected')).content).toEqual([{ type: 'text', text: 'still connected' }]);
    });

    it('reports readiness and drops all sessions on shutdown', async () => {
        const url = `http://127.0.0.1:${port}/healthz`;
        const starting = await fetch(url);
        expect(starting.status).toBe(503);
        await starting.body?.cancel();
        ready = true;
        const healthy = await fetch(url);
        expect(healthy.status).toBe(200);
        await healthy.body?.cancel();
        ready = false;
        const disconnected = await fetch(url);
        expect(disconnected.status).toBe(503);
        await disconnected.body?.cancel();

        await connect();
        await connect();
        await transport.stop();
        await expect(fetch(url)).rejects.toThrow();
    });
});
