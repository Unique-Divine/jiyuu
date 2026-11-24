import { Client, GatewayIntentBits } from 'discord.js';
import { config as dotenvConfig } from 'dotenv';
import { DiscordMCPServer, isDiscordReady } from './server.js';
import { StdioTransport, StreamableHttpTransport } from './transport.js';
import { info, error } from './logger.js';
import { isLoopbackHost, resolveHttpHost } from './httpHost.js';

export async function runDiscordMcp({ legacyApp = false } = {}) {
    const args = process.argv.slice(2);
    function option(name: string): string | undefined {
        const index = args.indexOf(name);
        return index === -1 ? undefined : args[index + 1];
    }

    const transportName = (option('--transport') ?? 'stdio').toLowerCase();
    const shared = args.includes('--shared');
    if (shared && (transportName !== 'http' || args.includes('--config'))) {
        error('--shared requires --transport http and DISCORD_TOKEN in the environment');
        process.exit(1);
    }

    if (!shared) dotenvConfig();
    const configuredToken = option('--config');
    function discordToken(): string | undefined {
        if (configuredToken !== undefined) {
            try {
                return JSON.parse(configuredToken).DISCORD_TOKEN;
            } catch {
                return configuredToken;
            }
        }
        return process.env.DISCORD_TOKEN;
    }

    const token = discordToken();
    const host = resolveHttpHost(args, process.env);
    const requestedPort = Number.parseInt(option('--port') ?? '8080', 10);
    const port = legacyApp && !shared ? requestedPort || 8080 : requestedPort;
    if (shared && (!token?.trim() || !isLoopbackHost(host))) {
        error('Shared HTTP requires DISCORD_TOKEN in the environment and a loopback host');
        process.exit(1);
    }
    if (transportName === 'http' && (!Number.isInteger(port) || port < 1 || port > 65535)) {
        error('HTTP port must be between 1 and 65535');
        process.exit(1);
    }

    const client = new Client({
        intents: [
            GatewayIntentBits.Guilds,
            GatewayIntentBits.GuildMessages,
            GatewayIntentBits.MessageContent,
            ...(legacyApp ? [] : [GatewayIntentBits.GuildMembers])
        ]
    });
    const isHttp = transportName === 'http';
    if (!isHttp && transportName !== 'stdio') {
        error(`Unknown transport type: ${transportName}. Falling back to stdio.`);
    }
    const transport = isHttp
        ? new StreamableHttpTransport({ port, host, isReady: shared ? () => isDiscordReady(client) : undefined })
        : new StdioTransport();
    const mcpServer = new DiscordMCPServer(client, transport, undefined, shared);

    let shuttingDown = false;
    async function shutdown(reason: string, exitCode = 0) {
        if (shuttingDown) return;
        shuttingDown = true;
        info(`Shutting down (${reason})`);
        const force = setTimeout(() => process.exit(exitCode), 3000);
        force.unref();
        try {
            await mcpServer.stop();
        } catch {
            error('Could not stop the MCP server');
        }
        process.exit(exitCode);
    }

    process.on('SIGINT', () => void shutdown('SIGINT'));
    process.on('SIGTERM', () => void shutdown('SIGTERM'));
    if (!isHttp) {
        process.stdin.on('close', () => void shutdown('stdin close'));
        process.stdin.on('end', () => void shutdown('stdin end'));
    }

    async function login() {
        if (!token) {
            info('No Discord token found in config, skipping auto-login');
            return;
        }
        client.token = token;
        try {
            await client.login(token);
            info('Successfully logged in to Discord');
        } catch (err) {
            if (err instanceof Error && err.message.includes('Privileged intent provided is not enabled or whitelisted')) {
                error('Login failed: Enable the required privileged intents in the Discord Developer Portal.');
            } else {
                error('Could not log in to Discord');
            }
            if (shared) await shutdown('Discord login failed', 1);
        }
    }

    try {
        // Shared readiness probes and legacy app clients can connect while Discord logs in.
        const loginBeforeStart = !shared && !legacyApp;
        if (loginBeforeStart) await login();
        await mcpServer.start();
        info('MCP server started successfully');
        if (!loginBeforeStart) await login();
    } catch (err) {
        // Node system error codes identify bind failures without including credentials.
        const code = (err as NodeJS.ErrnoException)?.code;
        error(typeof code === 'string' && /^E[A-Z0-9_]+$/.test(code)
            ? `Could not start the MCP server (${code})`
            : 'Could not start the MCP server');
        await shutdown('startup failed', 1);
    }
}
