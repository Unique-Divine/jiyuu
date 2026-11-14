import { describe, it, expect } from 'bun:test';
import { postInitialize, type RunningEntrypoint } from './httpEntrypoint.js';

/** Shared Host and Origin checks for an HTTP entrypoint. */
export function describeRebindingGuard(name: string, server: () => RunningEntrypoint) {
    describe(`${name} Host and Origin validation`, () => {
        it('accepts a loopback request without Origin', async () => {
            const res = await postInitialize(server().port);
            await res.body?.cancel();
            expect(res.status).toBe(200);
        });

        it('accepts a matching loopback Origin', async () => {
            const { port } = server();
            const res = await postInitialize(port, {
                host: `localhost:${port}`, origin: `http://localhost:${port}`
            });
            await res.body?.cancel();
            expect(res.status).toBe(200);
        });

        it('rejects a rebound Host with 403', async () => {
            const { port } = server();
            const res = await postInitialize(port, { host: `attacker.example:${port}` });
            expect(res.status).toBe(403);
            expect(await res.text()).toContain('Host header not allowed');
        });

        it('rejects a foreign Origin with 403', async () => {
            const { port } = server();
            const res = await postInitialize(port, { origin: 'http://attacker.example' });
            expect(res.status).toBe(403);
            expect(await res.text()).toContain('Origin header not allowed');
        });
    });
}
