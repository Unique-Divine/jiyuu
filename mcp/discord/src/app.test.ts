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
        await server.waitFor(/without session ID: method="tools\/call"/);
        expect(server.stderr()).not.toContain(secret);
    });
});

describeRebindingGuard('app.ts', () => server);
