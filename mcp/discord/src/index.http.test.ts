import { beforeAll, afterAll } from 'bun:test';
import { join } from 'path';
import { startHttpEntrypoint, type RunningEntrypoint } from './test-support/httpEntrypoint.js';
import { describeRebindingGuard } from './test-support/rebinding.js';

// Starts src/index.ts --transport http, which serves StreamableHttpTransport
// from transport.ts, with no Discord token.
let server: RunningEntrypoint;

beforeAll(async () => {
    server = startHttpEntrypoint(join(import.meta.dir, 'index.ts'));
    await server.waitFor(/MCP Server listening on/);
});

afterAll(async () => {
    await server.stop();
});

describeRebindingGuard('index.ts', () => server);
