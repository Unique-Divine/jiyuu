// Bun preload for HTTP entrypoint tests. No gateway or REST endpoints are used.
import { Client, GatewayIntentBits, Status, type WebSocketShard } from 'discord.js';

let fixtureClient: Client;
Client.prototype.login = async function (token) {
    fixtureClient = this;
    process.stderr.write('fixture login started\n');
    process.stderr.write(`fixture GuildMembers intent: ${this.options.intents.has(GatewayIntentBits.GuildMembers)}\n`);
    if (process.env.FIXTURE_LOGIN_FAIL) throw Error(`fixture login rejected ${token}`);
    await new Promise<void>(resolve => process.once('SIGUSR1', resolve));
    this.ws.status = Status.Ready;
    const shard = { status: Status.Ready } satisfies Pick<WebSocketShard, 'status'>;
    this.ws.shards.set(0, shard as WebSocketShard);
    return token ?? '';
};
Client.prototype.destroy = async function () {
    process.stderr.write('fixture client destroyed\n');
    this.ws.status = Status.Disconnected;
};
process.on('SIGUSR2', () => {
    const shard = fixtureClient.ws.shards.get(0);
    if (!shard) return;
    const reconnecting = shard.status === Status.Ready;
    shard.status = reconnecting ? Status.Connecting : Status.Ready;
    process.stderr.write(`fixture ${reconnecting ? 'reconnecting' : 'resumed'} (client.isReady()=${fixtureClient.isReady()})\n`);
});
