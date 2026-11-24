import { describe, expect, it } from 'bun:test';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { startHttpEntrypoint } from './test-support/httpEntrypoint.js';

const entry = join(import.meta.dir, 'index.ts');
const preload = join(import.meta.dir, 'test-support/discordLogin.ts');
const dummyToken = 'fixture-discord-token';

it('keeps runtime login on the stdio entrypoint', async () => {
    const cwd = mkdtempSync(join(tmpdir(), 'discord-stdio-'));
    const client = new Client({ name: 'stdio fixture', version: '1' });
    try {
        await client.connect(new StdioClientTransport({
            command: process.execPath,
            args: ['--no-env-file', entry],
            cwd,
            env: { JIYUU_MCP_NO_UPDATE_CHECK: '1' },
            stderr: 'ignore'
        }));
        const { tools } = await client.listTools();
        expect(tools.some(tool => tool.name === 'discord_login')).toBeTrue();
        expect(tools.some(tool => tool.name === 'mcp_info')).toBeTrue();
    } finally {
        await client.close();
        rmSync(cwd, { recursive: true, force: true });
    }
});

describe('shared HTTP CLI', () => {
    it('requires an environment token and rejects a command-line override or non-loopback host', async () => {
        const invalid = [
            startHttpEntrypoint(entry, {}, ['--shared']),
            startHttpEntrypoint(entry, {}, ['--shared', '--config', dummyToken]),
            startHttpEntrypoint(entry, { DISCORD_TOKEN: dummyToken }, ['--shared', '--host', '0.0.0.0']),
        ];
        try {
            for (const server of invalid) {
                expect(await server.exitCode).toBe(1);
                await server.waitFor(/DISCORD_TOKEN in the environment/);
                expect(server.stderr()).not.toContain(dummyToken);
            }
        } finally {
            await Promise.all(invalid.map(server => server.stop()));
        }
    });

    it('keeps one startup identity and changes readiness as the gateway connects and disconnects', async () => {
        const server = startHttpEntrypoint(entry, { DISCORD_TOKEN: dummyToken }, ['--shared'], ['--preload', preload]);
        const first = new Client({ name: 'first', version: '1' });
        const second = new Client({ name: 'second', version: '1' });
        const endpoint = `http://127.0.0.1:${server.port}`;
        async function health() {
            const res = await fetch(endpoint + '/healthz');
            await res.body?.cancel();
            return res.status;
        }

        try {
            await server.waitFor(/fixture login started/);
            await server.waitFor(/fixture GuildMembers intent: true/);
            expect(await health()).toBe(503);

            for (const client of [first, second]) {
                await client.connect(new StreamableHTTPClientTransport(new URL(endpoint + '/mcp')));
                const { tools } = await client.listTools();
                expect(tools.some(tool => tool.name === 'discord_login')).toBeFalse();
                expect(tools.some(tool => tool.name === 'mcp_info')).toBeTrue();
            }
            const denied = await first.callTool({ name: 'discord_login', arguments: { token: 'new-fixture-token' } });
            expect(denied.isError).toBe(true);
            expect(denied.content).toEqual([{ type: 'text', text: 'Discord login is configured by the shared service' }]);
            expect((await second.callTool({ name: 'discord_list_servers' })).isError).toBe(true);

            server.signal('SIGUSR1');
            await server.waitFor(/Successfully logged in/);
            expect(await health()).toBe(200);
            server.signal('SIGUSR2');
            await server.waitFor(/fixture reconnecting \(client\.isReady\(\)=true\)/);
            expect(await health()).toBe(503);
            const unavailable = await second.callTool({ name: 'discord_list_servers' });
            expect(unavailable.content).toEqual([{ type: 'text', text: 'Discord is not ready' }]);
            server.signal('SIGUSR2');
            await server.waitFor(/fixture resumed \(client\.isReady\(\)=true\)/);
            expect(await health()).toBe(200);

            server.signal('SIGTERM');
            expect(await server.exitCode).toBe(0);
            await server.waitFor(/fixture client destroyed/);
            expect(server.stderr().match(/fixture login started/g)).toHaveLength(1);
            expect(server.stderr().match(/fixture client destroyed/g)).toHaveLength(1);
            expect(server.stderr()).not.toContain(dummyToken);
        } finally {
            await Promise.allSettled([first.close(), second.close()]);
            await server.stop();
        }
    });

    it('exits on failed startup login without logging the token', async () => {
        const server = startHttpEntrypoint(entry, {
            DISCORD_TOKEN: dummyToken,
            FIXTURE_LOGIN_FAIL: '1'
        }, ['--shared'], ['--preload', preload]);
        try {
            expect(await server.exitCode).toBe(1);
            await server.waitFor(/fixture client destroyed/);
            expect(server.stderr()).toContain('Could not log in to Discord');
            expect(server.stderr()).not.toContain(dummyToken);
        } finally {
            await server.stop();
        }
    });
});
