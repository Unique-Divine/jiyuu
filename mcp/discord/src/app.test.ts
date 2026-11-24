import { describe, it, expect, beforeAll, afterAll } from 'bun:test';
import { join } from 'path';
import { startHttpEntrypoint, type RunningEntrypoint } from './test-support/httpEntrypoint.js';
import { describeRebindingGuard } from './test-support/rebinding.js';

// Starts the real HTTP entrypoint src/app.ts with no Discord token.
let server: RunningEntrypoint;

beforeAll(async () => {
    server = startHttpEntrypoint(join(import.meta.dir, 'app.ts'));
    await server.waitFor(/listening on/);
});

afterAll(async () => {
    await server.stop();
});

describe('app.ts HTTP entrypoint', () => {
    it('binds to loopback by default', async () => {
        const [, address] = await server.waitFor(/listening on (\S+)\n/);
        expect(address).toBe(`127.0.0.1:${server.port}`);
        expect(server.stderr()).not.toContain('[warning]');
    });

    it('does not log request bodies for requests without a session', async () => {
        const secret = 'sentinel-token-value-4f9c';
        const res = await fetch(`http://127.0.0.1:${server.port}/mcp`, {
            method: 'POST',
            headers: { 'content-type': 'application/json', accept: 'application/json, text/event-stream' },
            body: JSON.stringify({
                jsonrpc: '2.0', id: 7, method: 'tools/call',
                params: { name: 'discord_login', arguments: { token: secret } }
            })
        });
        expect(res.status).toBe(400);
        expect(await res.text()).toContain("Missing MCP session ID");
        expect(server.stderr()).not.toContain(secret);
    });

    it('rejects malformed JSON without reflecting request text', async () => {
        const sentinel = 'sensitive-fixture-value';
        const res = await fetch(`http://127.0.0.1:${server.port}/mcp`, {
            method: 'POST',
            headers: { 'content-type': 'application/json', accept: 'application/json, text/event-stream' },
            body: `{ "token": ${sentinel} }`
        });
        expect(res.status).toBe(400);
        expect(await res.json()).toEqual({
            jsonrpc: '2.0', error: { code: -32000, message: 'Invalid MCP request body' }, id: null
        });
        expect(server.stderr()).not.toContain(sentinel);
    });

    it('keeps the original gateway intents and listens during login', async () => {
        const legacy = startHttpEntrypoint(join(import.meta.dir, 'app.ts'), { DISCORD_TOKEN: 'fixture' }, [], [
            '--preload', join(import.meta.dir, 'test-support/discordLogin.ts')
        ]);
        try {
            await legacy.waitFor(/listening on/);
            await legacy.waitFor(/fixture GuildMembers intent: false/);
            legacy.signal('SIGUSR1');
            await legacy.waitFor(/Successfully logged in/);
        } finally {
            await legacy.stop();
        }
    });
});

describeRebindingGuard('app.ts', () => server);
