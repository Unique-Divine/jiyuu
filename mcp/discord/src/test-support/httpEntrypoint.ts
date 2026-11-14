import { mkdtempSync } from 'fs';
import { tmpdir } from 'os';
import { join } from 'path';

export interface RunningEntrypoint {
    port: number;
    stderr(): string;
    waitFor(pattern: RegExp, timeoutMs?: number): Promise<RegExpMatchArray>;
    stop(): Promise<void>;
}

function freePort(): number {
    const probe = Bun.listen({
        hostname: '127.0.0.1', port: 0,
        socket: { data() {} }
    });
    const port = probe.port;
    probe.stop(true);
    return port;
}

/**
 * Start an entrypoint with `--transport http` on a free port, no Discord
 * token and a minimal environment, so no secrets reach the child and it
 * never logs in. The working directory is empty, so no .env is loaded.
 */
export function startHttpEntrypoint(
    entry: string,
    env: Record<string, string> = {}
): RunningEntrypoint {
    const workDir = mkdtempSync(join(tmpdir(), 'mcp-discord-http-'));
    const port = freePort();
    const child = Bun.spawn(
        [process.execPath, entry, '--transport', 'http', '--port', String(port)],
        {
            cwd: workDir,
            env: { PATH: process.env.PATH ?? '/usr/bin:/bin', HOME: workDir, ...env },
            // index.ts shuts down when stdin ends, so keep it open.
            stdin: 'pipe', stdout: 'ignore', stderr: 'pipe'
        }
    );
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
    return {
        port,
        stderr: () => stderr,
        async waitFor(pattern, timeoutMs = 10000) {
            const deadline = Date.now() + timeoutMs;
            for (;;) {
                const match = stderr.match(pattern);
                if (match) return match;
                if (Date.now() > deadline) {
                    throw new Error(`Timed out waiting for ${pattern}; stderr:\n${stderr}`);
                }
                await Bun.sleep(25);
            }
        },
        async stop() {
            child.kill('SIGKILL');
            await child.exited;
            await pump.catch(() => {});
        }
    };
}

export const initializeBody = JSON.stringify({
    jsonrpc: '2.0', id: 1, method: 'initialize',
    params: {
        protocolVersion: '2025-03-26', capabilities: {},
        clientInfo: { name: 'test', version: '0' }
    }
});

/** POST an initialize request with optional Host and Origin overrides. */
export async function postInitialize(
    port: number,
    headers: Record<string, string> = {}
): Promise<Response> {
    const res = await fetch(`http://127.0.0.1:${port}/mcp`, {
        method: 'POST',
        headers: {
            'content-type': 'application/json',
            accept: 'application/json, text/event-stream',
            ...headers
        },
        body: initializeBody
    });
    return res;
}
