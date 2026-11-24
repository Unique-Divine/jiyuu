// Bun preload for HTTP entrypoint tests. No gateway or REST endpoints are used.
import { Client } from 'discord.js';

let ready = false;
Client.prototype.isReady = (() => ready) as Client['isReady'];
Client.prototype.login = async function (token) {
    process.stderr.write('fixture login started\n');
    if (process.env.FIXTURE_LOGIN_FAIL) throw Error(`fixture login rejected ${token}`);
    await new Promise<void>(resolve => process.once('SIGUSR1', resolve));
    ready = true;
    return token ?? '';
};
Client.prototype.destroy = async function () {
    process.stderr.write('fixture client destroyed\n');
    ready = false;
};
process.on('SIGUSR2', () => {
    ready = false;
    process.stderr.write('fixture disconnected\n');
});
