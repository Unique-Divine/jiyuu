// Starts the real HTTP entrypoints in a child process. transport.test.ts
// mocks express and the SDK, so only a running server shows what the Host
// and Origin guard, the bound address and the logs actually do.
import { mkdtempSync, rmSync } from 'fs';
import { tmpdir } from 'os';
import { join } from 'path';

export interface RunningEntrypoint {
    port: number;
    stderr(): string;
    waitFor(pattern: RegExp, timeoutMs?: number): Promise<RegExpMatchArray>;
    signal(signal: NodeJS.Signals): void;
    exitCode: Promise<number>;
    stop(): Promise<void>;
}

export function freePort(): number {
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
 * never logs in. The working directory is empty, so no .env is loaded, and
 * the update check is off.
 */
export function startHttpEntrypoint(
    entry: string,
    env: Record<string, string> = {},
    args: string[] = [],
    bunArgs: string[] = [],
): RunningEntrypoint {
    const workDir = mkdtempSync(join(tmpdir(), 'mcp-discord-http-'));
    const port = freePort();
    const child = Bun.spawn(
        [process.execPath, '--no-env-file', ...bunArgs, entry, '--transport', 'http', '--port', String(port), ...args],
        {
            cwd: workDir,
            env: {
                PATH: process.env.PATH ?? '/usr/bin:/bin',
                HOME: workDir,
                // Keep tests off the npm registry.
                JIYUU_MCP_NO_UPDATE_CHECK: '1',
                ...env,
            },
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
        signal: signal => child.kill(signal),
        exitCode: child.exited,
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
            rmSync(workDir, { recursive: true, force: true });
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
