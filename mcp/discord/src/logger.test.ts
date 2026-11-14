import { describe, it, expect, beforeEach, afterEach, mock } from 'bun:test';
import { log, info, error } from './logger.js';

// Upstream commit 34882e8 moved diagnostics from JSON-RPC "log" messages on
// stdout to plain lines on stderr, because stdout carries the MCP stdio
// protocol. These tests follow that behavior.
const mockStdoutWrite = mock(() => true);
const mockStderrWrite = mock(() => true);

describe('Logger', () => {
    let originalStdoutWrite: typeof process.stdout.write;
    let originalStderrWrite: typeof process.stderr.write;

    beforeEach(() => {
        originalStdoutWrite = process.stdout.write;
        originalStderrWrite = process.stderr.write;
        process.stdout.write = mockStdoutWrite as any;
        process.stderr.write = mockStderrWrite as any;
        mockStdoutWrite.mockClear();
        mockStderrWrite.mockClear();
    });

    afterEach(() => {
        process.stdout.write = originalStdoutWrite;
        process.stderr.write = originalStderrWrite;
    });

    function written(): string[] {
        return mockStderrWrite.mock.calls.map((call: any[]) => call[0]);
    }

    describe('log', () => {
        it('should log with default info level when no level is provided', () => {
            log('Test message');
            expect(written()).toEqual(['[info] Test message\n']);
        });

        it('should log with specified info level', () => {
            log('Info message', 'info');
            expect(written()).toEqual(['[info] Info message\n']);
        });

        it('should log with specified error level', () => {
            log('Error message', 'error');
            expect(written()).toEqual(['[error] Error message\n']);
        });

        it('should never write to stdout, the MCP stdio protocol channel', () => {
            log('Test message', 'info');
            log('Test message', 'error');
            expect(mockStdoutWrite).not.toHaveBeenCalled();
        });
    });

    describe('info', () => {
        it('should call log with info level', () => {
            info('Info message');
            expect(written()).toEqual(['[info] Info message\n']);
        });
    });

    describe('error', () => {
        it('should call log with error level', () => {
            error('Error message');
            expect(written()).toEqual(['[error] Error message\n']);
        });
    });

    describe('output formatting', () => {
        it('should append newline character to the output', () => {
            log('Test message', 'info');
            expect(written()[0]).toMatch(/\n$/);
        });
    });
});
