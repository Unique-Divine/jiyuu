import { describe, it, expect, afterAll } from 'bun:test';
import { mkdtempSync } from 'fs';
import { tmpdir } from 'os';
import { join } from 'path';

// Starts the real HTTP entrypoint with no Discord token and a minimal
// environment, so no secrets reach the child and it never logs in.
const entry = join(import.meta.dir, 'app.ts');
const workDir = mkdtempSync(join(tmpdir(), 'mcp-discord-app-'));

function freePort(): number {
    const probe = Bun.listen({
        hostname: '127.0.0.1', port: 0,
        socket: { data() {} }
    });
    const port = probe.port;
    probe.stop(true);
    return port;
}

const port = freePort();
const child = Bun.spawn([process.execPath, entry, '--transport', 'http', '--port', String(port)], {
    cwd: workDir,
    env: { PATH: process.env.PATH ?? '/usr/bin:/bin', HOME: workDir },
    stdin: 'ignore', stdout: 'ignore', stderr: 'pipe'
});

let stderr = '';
const decoder = new TextDecoder();
const reader = child.stderr.getReader();
const pump = (async () => {
    for (;;) {
        const { value, done } = await reader.read();
        if (done) return;
        stderr += decoder.decode(value);
    }
})();

async function waitFor(pattern: RegExp, timeoutMs = 10000): Promise<RegExpMatchArray> {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
        const match = stderr.match(pattern);
        if (match) return match;
        if (Date.now() > deadline) throw new Error(`Timed out waiting for ${pattern}; stderr:\n${stderr}`);
        await Bun.sleep(25);
    }
}

afterAll(async () => {
    child.kill('SIGKILL');
    await child.exited;
    await pump.catch(() => {});
});

describe('app.ts HTTP entrypoint', () => {
    it('binds to loopback by default', async () => {
        const [, address] = await waitFor(/listening on (\S+)\n/);
        expect(address).toBe(`127.0.0.1:${port}`);
        expect(stderr).not.toContain('[warning]');
    });

    it('does not log request bodies for requests without a session', async () => {
        await waitFor(/listening on/);
        const secret = 'sentinel-token-value-4f9c';
        const res = await fetch(`http://127.0.0.1:${port}/mcp`, {
            method: 'POST',
            headers: { 'content-type': 'application/json', accept: 'application/json, text/event-stream' },
            body: JSON.stringify({
                jsonrpc: '2.0', id: 7, method: 'tools/call',
                params: { name: 'discord_login', arguments: { token: secret } }
            })
        });
        expect(res.status).toBe(400);
        await waitFor(/without session ID: method="tools\/call"/);
        expect(stderr).not.toContain(secret);
    });
});
