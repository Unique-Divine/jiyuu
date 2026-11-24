import type { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { StreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/streamableHttp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { isInitializeRequest } from "@modelcontextprotocol/sdk/types.js";
import express, { type Request, type Response } from "express";
import { randomUUID } from "node:crypto";
import type { Server as HttpServer } from "node:http";
import { info, error } from './logger.js';
import {
    DEFAULT_HTTP_HOST,
    allowedHostKeys,
    extraAllowedHosts,
    hostOriginGuard,
    nonLoopbackWarning
} from './httpHost.js';

export interface MCPTransport {
    start(createServer: () => Server): Promise<void>;
    stop(): Promise<void>;
}

export class StdioTransport implements MCPTransport {
    private transport: StdioServerTransport | null = null;

    async start(createServer: () => Server): Promise<void> {
        this.transport = new StdioServerTransport();
        await createServer().connect(this.transport);
    }

    async stop(): Promise<void> {
        if (this.transport) {
            await this.transport.close();
            this.transport = null;
        }
    }
}

interface HttpSession {
    transport: StreamableHTTPServerTransport;
    lastActivity: number;
    activePosts: number;
}

interface HttpOptions {
    port?: number;
    host?: string;
    isReady?: () => boolean;
    idleTimeoutMs?: number;
}

const IDLE_TIMEOUT_MS = 30 * 60 * 1000;

function rpcError(res: Response, status: number, message: string) {
    res.status(status).json({ jsonrpc: '2.0', error: { code: -32000, message }, id: null });
}

export class StreamableHttpTransport implements MCPTransport {
    private readonly sessions = new Map<string, HttpSession>();
    private readonly transports = new Set<StreamableHTTPServerTransport>();
    private httpServer: HttpServer | null = null;
    private idleTimer: ReturnType<typeof setInterval> | null = null;
    private stopping = false;

    constructor(private readonly options: HttpOptions = {}) {}

    async start(createServer: () => Server): Promise<void> {
        const { port = 8080, host = DEFAULT_HTTP_HOST, isReady, idleTimeoutMs = IDLE_TIMEOUT_MS } = this.options;
        const app = express();
        app.use(hostOriginGuard(allowedHostKeys(host, port, extraAllowedHosts(process.env))));
        if (isReady) {
            app.get('/healthz', (_req, res) => {
                res.set('Cache-Control', 'no-store');
                res.sendStatus(this.stopping || !isReady() ? 503 : 200);
            });
        }
        app.use(express.json());
        app.all('/mcp', (req, res) => this.handleRequest(createServer, req, res));

        const warning = nonLoopbackWarning(host);
        if (warning) error(warning);
        const listener = app.listen(port, host);
        this.httpServer = listener;
        await new Promise<void>((resolve, reject) => {
            listener.once('error', reject);
            listener.once('listening', () => {
                listener.off('error', reject);
                resolve();
            });
        });
        info(`MCP Server listening on ${host}:${port}`);
        this.idleTimer = setInterval(() => {
            const cutoff = Date.now() - idleTimeoutMs;
            for (const session of this.sessions.values()) {
                if (session.activePosts === 0 && session.lastActivity <= cutoff) {
                    void this.closeTransport(session.transport);
                }
            }
        }, Math.min(idleTimeoutMs, 60_000));
        this.idleTimer.unref();
    }

    private async handleRequest(createServer: () => Server, req: Request, res: Response): Promise<void> {
        if (this.stopping) {
            rpcError(res, 503, 'Server is shutting down');
            return;
        }

        const sessionId = req.get('Mcp-Session-Id');
        let session = sessionId ? this.sessions.get(sessionId) : undefined;
        if (sessionId && !session) {
            rpcError(res, 404, 'Unknown MCP session');
            return;
        }
        if (!session && (req.method !== 'POST' || !isInitializeRequest(req.body))) {
            rpcError(res, 400, 'Missing MCP session ID');
            return;
        }

        let newSession: HttpSession | undefined;
        try {
            if (!session) {
                session = newSession = await this.createSession(createServer);
                if (this.stopping) {
                    rpcError(res, 503, 'Server is shutting down');
                    return;
                }
            }

            session.lastActivity = Date.now();
            if (req.method === 'POST') this.trackPost(session, res);
            await session.transport.handleRequest(req, res, req.body);
        } catch {
            error('Could not handle MCP request');
            if (!res.headersSent) rpcError(res, 500, 'Internal server error');
            else res.end();
        } finally {
            if (newSession && !newSession.transport.sessionId) {
                await this.closeTransport(newSession.transport);
            }
        }
    }

    private async createSession(createServer: () => Server): Promise<HttpSession> {
        const transport = new StreamableHTTPServerTransport({
            sessionIdGenerator: randomUUID,
            enableJsonResponse: true,
            onsessioninitialized: (id) => { this.sessions.set(id, session); }
        });
        const session: HttpSession = { transport, lastActivity: Date.now(), activePosts: 0 };
        transport.onclose = () => {
            if (transport.sessionId) this.sessions.delete(transport.sessionId);
            this.transports.delete(transport);
        };
        this.transports.add(transport);
        try {
            await createServer().connect(transport);
            return session;
        } catch (err) {
            await this.closeTransport(transport);
            throw err;
        }
    }

    private trackPost(session: HttpSession, res: Response) {
        session.activePosts += 1;
        const finish = () => {
            res.off('finish', finish);
            res.off('close', finish);
            session.activePosts -= 1;
            session.lastActivity = Date.now();
        };
        res.once('finish', finish);
        res.once('close', finish);
    }

    private async closeTransport(transport: StreamableHTTPServerTransport) {
        try {
            await transport.close();
        } catch {
            error('Could not close MCP session');
        }
    }

    async stop(): Promise<void> {
        this.stopping = true;
        if (this.idleTimer) {
            clearInterval(this.idleTimer);
            this.idleTimer = null;
        }
        const listener = this.httpServer;
        this.httpServer = null;
        const closed = new Promise<void>((resolve) => {
            if (!listener) return resolve();
            listener.close(() => resolve());
        });
        await Promise.all([...this.transports].map(transport => this.closeTransport(transport)));
        listener?.closeAllConnections();
        await closed;
    }
}
