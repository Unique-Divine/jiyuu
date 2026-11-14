import { describe, it, expect } from 'bun:test';
import {
    DEFAULT_HTTP_HOST,
    allowedHostKeys,
    checkHostAndOrigin,
    extraAllowedHosts,
    isLoopbackHost,
    nonLoopbackWarning,
    resolveHttpHost
} from './httpHost.js';

describe('resolveHttpHost', () => {
    it('defaults to loopback', () => {
        expect(DEFAULT_HTTP_HOST).toBe('127.0.0.1');
        expect(resolveHttpHost(['bun', 'src/app.ts'], {})).toBe('127.0.0.1');
        expect(resolveHttpHost([], { HTTP_HOST: '  ' })).toBe('127.0.0.1');
    });

    it('takes HTTP_HOST, and --host over HTTP_HOST', () => {
        expect(resolveHttpHost([], { HTTP_HOST: '0.0.0.0' })).toBe('0.0.0.0');
        expect(resolveHttpHost(['--host', '::1'], { HTTP_HOST: '0.0.0.0' }))
            .toBe('::1');
    });
});

describe('isLoopbackHost', () => {
    it('accepts loopback names and addresses', () => {
        for (const host of ['127.0.0.1', '127.1.2.3', 'localhost', '::1', '[::1]']) {
            expect(isLoopbackHost(host)).toBe(true);
        }
    });

    it('rejects wildcard and routable addresses', () => {
        for (const host of ['0.0.0.0', '::', '192.168.1.5', '100.64.0.1', 'example.com']) {
            expect(isLoopbackHost(host)).toBe(false);
        }
    });
});

describe('nonLoopbackWarning', () => {
    it('warns only for non-loopback hosts', () => {
        expect(nonLoopbackWarning('127.0.0.1')).toBeNull();
        expect(nonLoopbackWarning('0.0.0.0')).toContain('no authentication');
    });
});

describe('allowedHostKeys', () => {
    it('allows loopback names on the bound port', () => {
        expect(allowedHostKeys('127.0.0.1', 3000).sort())
            .toEqual(['127.0.0.1:3000', '[::1]:3000', 'localhost:3000']);
    });

    it('adds an explicit host and HTTP_ALLOWED_HOSTS, but not a wildcard', () => {
        expect(allowedHostKeys('100.64.0.1', 3000)).toContain('100.64.0.1:3000');
        expect(allowedHostKeys('0.0.0.0', 3000)).not.toContain('0.0.0.0:3000');
        const extra = extraAllowedHosts({ HTTP_ALLOWED_HOSTS: 'box.example:3000, other' });
        expect(allowedHostKeys('0.0.0.0', 3000, extra))
            .toEqual(expect.arrayContaining(['box.example:3000', 'other:3000']));
    });
});

describe('checkHostAndOrigin', () => {
    const allowed = allowedHostKeys('127.0.0.1', 3000);

    it('accepts loopback Host with no Origin or a matching Origin', () => {
        expect(checkHostAndOrigin({ host: '127.0.0.1:3000' }, allowed).ok).toBe(true);
        expect(checkHostAndOrigin({ host: 'LOCALHOST:3000', origin: 'http://localhost:3000' }, allowed).ok)
            .toBe(true);
        expect(checkHostAndOrigin({ host: '[::1]:3000' }, allowed).ok).toBe(true);
    });

    it('rejects missing, foreign or wrong-port Host', () => {
        for (const host of [undefined, 'attacker.example:3000', 'localhost:4000', 'localhost']) {
            expect(checkHostAndOrigin({ host }, allowed)).toEqual({ ok: false, reason: 'Host header not allowed' });
        }
    });

    it('rejects foreign, null or non-http Origin', () => {
        for (const origin of ['http://attacker.example', 'null', 'file://', 'http://localhost:4000']) {
            expect(checkHostAndOrigin({ host: '127.0.0.1:3000', origin }, allowed))
                .toEqual({ ok: false, reason: 'Origin header not allowed' });
        }
    });
});
