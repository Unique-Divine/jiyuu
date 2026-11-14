// Jiyuu local modification: the HTTP transports have no authentication, so
// they bind to loopback unless the operator names another host explicitly.

export const DEFAULT_HTTP_HOST = '127.0.0.1';

/** Host from `--host <value>`, then HTTP_HOST, then loopback. */
export function resolveHttpHost(
    argv: readonly string[],
    env: Record<string, string | undefined>
): string {
    const hostIndex = argv.indexOf('--host');
    if (hostIndex !== -1 && hostIndex + 1 < argv.length) {
        return argv[hostIndex + 1];
    }
    const fromEnv = env.HTTP_HOST?.trim();
    return fromEnv ? fromEnv : DEFAULT_HTTP_HOST;
}

export function isLoopbackHost(host: string): boolean {
    const h = host.trim().toLowerCase().replace(/^\[(.*)\]$/, '$1');
    return h === 'localhost' || h === '::1' || /^127(\.\d{1,3}){3}$/.test(h);
}

export function nonLoopbackWarning(host: string): string | null {
    if (isLoopbackHost(host)) return null;
    return `HTTP transport bound to non-loopback host ${host}. It has no ` +
        'authentication; any client that can reach this address can call ' +
        'every Discord tool.';
}

// DNS rebinding guard. A web page can point its own hostname at 127.0.0.1,
// so binding to loopback alone does not keep browsers out. Requests must
// name an allowed Host, and a browser Origin, when sent, must match one.
// Clients that send no Origin (MCP clients, curl) are allowed. The SDK's own
// check is not used because it rejects requests that omit Origin.

function hostKey(value: string, defaultPort: string): string | null {
    try {
        const url = new URL(`http://${value.trim()}`);
        if (url.username || url.password || url.pathname !== '/') return null;
        return `${url.hostname.toLowerCase()}:${url.port || defaultPort}`;
    } catch {
        return null;
    }
}

function originKey(origin: string): string | null {
    try {
        const url = new URL(origin);
        if (url.protocol !== 'http:' && url.protocol !== 'https:') return null;
        const port = url.port || (url.protocol === 'https:' ? '443' : '80');
        return `${url.hostname.toLowerCase()}:${port}`;
    } catch {
        return null;
    }
}

/** Host:port values a request may name for a server bound to host:port. */
export function allowedHostKeys(
    host: string,
    port: number,
    extraHosts: readonly string[] = []
): string[] {
    const keys = new Set([`127.0.0.1:${port}`, `localhost:${port}`, `[::1]:${port}`]);
    const bound = host.includes(':') && !host.startsWith('[') ? `[${host}]` : host;
    const wildcard = bound === '0.0.0.0' || bound === '[::]';
    if (!wildcard) {
        const key = hostKey(`${bound}:${port}`, String(port));
        if (key) keys.add(key);
    }
    for (const extra of extraHosts) {
        const key = hostKey(extra, String(port));
        if (key) keys.add(key);
    }
    return [...keys];
}

/** Comma-separated extra Host values from HTTP_ALLOWED_HOSTS. */
export function extraAllowedHosts(env: Record<string, string | undefined>): string[] {
    return (env.HTTP_ALLOWED_HOSTS ?? '')
        .split(',').map((h) => h.trim()).filter((h) => h.length > 0);
}

export type HeaderCheck = { ok: true } | { ok: false; reason: string };

export function checkHostAndOrigin(
    headers: { host?: string; origin?: string },
    allowed: readonly string[]
): HeaderCheck {
    const host = headers.host ? hostKey(headers.host, '80') : null;
    if (!host || !allowed.includes(host)) {
        return { ok: false, reason: 'Host header not allowed' };
    }
    if (headers.origin !== undefined) {
        const origin = originKey(headers.origin);
        if (!origin || !allowed.includes(origin)) {
            return { ok: false, reason: 'Origin header not allowed' };
        }
    }
    return { ok: true };
}

type GuardRequest = { headers: Record<string, string | string[] | undefined> };
type GuardResponse = { status(code: number): { json(body: unknown): unknown } };

/** Express middleware that answers 403 for disallowed Host or Origin. */
export function hostOriginGuard(allowed: readonly string[]) {
    return (req: GuardRequest, res: GuardResponse, next: () => void): void => {
        const first = (v: string | string[] | undefined) => Array.isArray(v) ? v[0] : v;
        const check = checkHostAndOrigin(
            { host: first(req.headers.host), origin: first(req.headers.origin) },
            allowed
        );
        if (check.ok) {
            next();
            return;
        }
        res.status(403).json({
            jsonrpc: '2.0',
            error: { code: -32000, message: `Forbidden: ${check.reason}` },
            id: null
        });
    };
}
