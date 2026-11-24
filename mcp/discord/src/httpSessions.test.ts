import { afterEach, beforeEach, describe, expect, it } from 'bun:test';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';
import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import {
    CallToolResultSchema, InitializeResultSchema, JSONRPCRequestSchema, JSONRPCResponseSchema, ListToolsResultSchema
} from '@modelcontextprotocol/sdk/types.js';
import { z } from 'zod';
import { StreamableHttpTransport } from './transport.js';
import { freePort, postInitialize } from './test-support/httpEntrypoint.js';

const methods = z.enum(['initialize', 'tools/list', 'tools/call']);
const exchangeSchema = z.object({
    method: methods,
    requestSession: z.string().nullable(),
    responseSession: z.string().nullable(),
    response: JSONRPCResponseSchema,
});
type Exchange = z.infer<typeof exchangeSchema>;
const resultSchemas = {
    initialize: InitializeResultSchema,
    'tools/list': ListToolsResultSchema,
    'tools/call': CallToolResultSchema,
};

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

    async function connect(recordAs?: 'client-a' | 'client-b') {
        const client = new Client({ name: 'fixture client', version: '1' });
        clients.push(client);
        const exchanges: Exchange[] = [];
        let session: string | null = null;
        function normalize(id: string | null): string | null {
            if (id === null) return null;
            if (id !== session || !recordAs) throw new Error('Unexpected session ID');
            return recordAs;
        }
        const connection = new StreamableHTTPClientTransport(new URL(`http://127.0.0.1:${port}/mcp`), {
            fetch: async (input, init) => {
                const response = await fetch(input, init);
                if (recordAs && typeof init?.body === 'string') {
                    const request = JSONRPCRequestSchema.safeParse(JSON.parse(init.body));
                    const method = methods.safeParse(request.data?.method);
                    if (request.success && method.success) {
                        const responseId = response.headers.get('Mcp-Session-Id');
                        if (method.data === 'initialize') session = responseId;
                        const rpcResponse = JSONRPCResponseSchema.parse(await response.clone().json());
                        expect(rpcResponse.id).toBe(request.data.id);
                        exchanges.push({
                            method: method.data,
                            requestSession: normalize(new Headers(init.headers).get('Mcp-Session-Id')),
                            responseSession: normalize(responseId),
                            response: rpcResponse,
                        });
                    }
                }
                return response;
            }
        });
        await client.connect(connection);
        return { client, connection, exchanges };
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
        const { client: first, connection: firstConnection, exchanges: firstExchanges } = await connect('client-a');
        const { client: second, connection: secondConnection, exchanges: secondExchanges } = await connect('client-b');
        expect(firstConnection.sessionId).toBeString();
        expect(secondConnection.sessionId).not.toBe(firstConnection.sessionId);

        // Both SDK clients start at the same initial request ID.
        await first.listTools();
        await second.listTools();
        const delayed = echo(first, 'delayed');
        await entered.promise;
        expect((await echo(second, 'immediate')).content).toEqual([{ type: 'text', text: 'immediate' }]);
        release.resolve();
        expect((await delayed).content).toEqual([{ type: 'text', text: 'delayed' }]);
        expect(firstExchanges.at(-1)?.response.id).toBe(secondExchanges.at(-1)?.response.id);
        const recording = [...firstExchanges, ...secondExchanges];
        const file = Bun.file(new URL('./test-support/fixtures/http-sessions.json', import.meta.url));
        const replay = exchangeSchema.array().length(6).parse(await file.json());
        for (const { method, response } of replay) resultSchemas[method].parse(response.result);
        expect(recording).toEqual(replay);
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
