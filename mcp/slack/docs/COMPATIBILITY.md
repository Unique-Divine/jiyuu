# Compatibility Matrix

Use this matrix to choose a known working client/runtime path before rollout.

| Client | Mode | Token path | Status | Quick verify command |
|---|---|---|---|---|
| Claude Desktop (macOS) | `stdio` | `~/.slack-mcp-tokens.json` via `--setup` auto-extract | Supported | `bunx @uniquedivine/mcp-slack@latest --setup && bunx @uniquedivine/mcp-slack@latest --status` |
| Claude Desktop (Windows) | `stdio` | `env` (`SLACK_TOKEN`, `SLACK_COOKIE`) in config | Supported | `bunx @uniquedivine/mcp-slack@latest --status` |
| Claude Desktop (Linux) | `stdio` | `env` or token file via guided setup | Supported | `bunx @uniquedivine/mcp-slack@latest --status` |
| Claude Code CLI | `stdio` | `~/.slack-mcp-tokens.json` or `env` | Supported | `bunx @uniquedivine/mcp-slack@latest --version && bunx @uniquedivine/mcp-slack@latest --status` |
| Local Browser UI | `web` | token file or `env` | Supported | `bunx @uniquedivine/mcp-slack@latest web` |
| Hosted Bun Runtime | `http` | `env` in host runtime | Supported with operator controls | `bun src/server-http.js` then `curl -s http://localhost:8080/health` |
| Cloudflare Worker / Smithery transport | `worker` | runtime env/query handoff per deployment config | Supported with deployment validation | `wrangler deploy --config workers/wrangler.toml` and verify `/health` |

## Notes

1. Runtime baseline is Node 20+.
2. `--doctor` is the fastest first check when setup status is unknown.
3. For hosted/team deployment, use [DEPLOYMENT-MODES.md](DEPLOYMENT-MODES.md) before production rollout.
