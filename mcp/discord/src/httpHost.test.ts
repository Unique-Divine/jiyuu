import { describe, it, expect } from 'bun:test';
import {
    DEFAULT_HTTP_HOST,
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
