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
