import { describe, it, expect, beforeEach, afterEach, jest, mock } from 'bun:test';
import type { Server } from '@modelcontextprotocol/sdk/server/index.js';

const StdioServerTransportMock = jest.fn();
mock.module('@modelcontextprotocol/sdk/server/stdio.js', () => ({
  StdioServerTransport: StdioServerTransportMock
}));

const { StdioTransport } = await import('./transport.js');

describe('StdioTransport', () => {
  let stdioTransport: InstanceType<typeof StdioTransport>;
  let mockServer: Server;

  beforeEach(() => {
    stdioTransport = new StdioTransport();
    mockServer = {
      connect: jest.fn().mockResolvedValue(undefined),
      close: jest.fn().mockResolvedValue(undefined)
    } as unknown as Server;
  });

  afterEach(() => {
    jest.clearAllMocks();
  });

  describe('start', () => {
    it('should initialize StdioServerTransport and connect to server', async () => {
      const mockStdioTransport = {
        close: jest.fn().mockResolvedValue(undefined)
      };
      StdioServerTransportMock
        .mockReturnValue(mockStdioTransport);

      await stdioTransport.start(() => mockServer);

      expect(StdioServerTransportMock).toHaveBeenCalledTimes(1);
      expect(mockServer.connect).toHaveBeenCalledWith(mockStdioTransport);
    });
  });

  describe('stop', () => {
    it('should close the transport if it exists', async () => {
      const mockStdioTransport = {
        close: jest.fn().mockResolvedValue(undefined)
      };
      StdioServerTransportMock
        .mockReturnValue(mockStdioTransport);

      // Start the transport first to initialize it
      await stdioTransport.start(() => mockServer);
      await stdioTransport.stop();

      expect(mockStdioTransport.close).toHaveBeenCalled();
    });

    it('should do nothing if transport is not initialized', async () => {
      await stdioTransport.stop();

      expect(stdioTransport).toBeDefined(); // Transport exists but no-op
    });
  });
});
