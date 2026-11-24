// Retain the original app's gateway permissions on the shared implementation.
import { runDiscordMcp } from './cli.js';

await runDiscordMcp({ legacyApp: true });
