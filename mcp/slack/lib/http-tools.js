/**
 * Tool Surface for the HTTP Transport
 *
 * The stdio server runs inside a local client such as Claude Code, which asks
 * the user before each call to a tool it hasn't been told to allow. HTTP
 * clients of src/server-http.js may never prompt. So over HTTP the _unsafe
 * tools, which skip the edit and delete age window, are neither listed nor
 * dispatched unless the operator sets SLACK_MCP_ENABLE_UNSAFE_TOOLS=true.
 */

import { TOOLS } from "./tools.js";
import { TOOL_HANDLERS } from "./handlers.js";
import { isAuthDeath, lifeboatResponse } from "./lifeboat.js";

export const UNSAFE_TOOLS_ENV = "SLACK_MCP_ENABLE_UNSAFE_TOOLS";

export function isUnsafeTool(name) {
  return typeof name === "string" && name.endsWith("_unsafe");
}

export function unsafeToolsEnabled(env = process.env) {
  return env[UNSAFE_TOOLS_ENV] === "true";
}

function errorResult(code, message, nextAction) {
  return {
    content: [{
      type: "text",
      text: JSON.stringify({
        status: "error",
        code,
        message,
        next_action: nextAction
      }, null, 2)
    }],
    isError: true
  };
}

/**
 * List and call handlers for the HTTP transport. `env` is read once, when the
 * handlers are built.
 */
export function createHttpToolHandlers(env = process.env) {
  const allowUnsafe = unsafeToolsEnabled(env);
  const tools = allowUnsafe ? TOOLS : TOOLS.filter((t) => !isUnsafeTool(t.name));

  async function listTools() {
    return { tools };
  }

  async function callTool(request) {
    const { name, arguments: args } = request.params;

    try {
      // Shared dispatch map keeps this transport in lockstep with the stdio
      // server and the advertised TOOLS list.
      const handler = TOOL_HANDLERS[name];
      if (!handler || (isUnsafeTool(name) && !allowUnsafe)) {
        return errorResult(
          "unknown_tool",
          `Unknown tool: ${name}`,
          "Call tools/list to inspect available tool names."
        );
      }
      return await handler(args);
    } catch (error) {
      // OAuth Lifeboat: dead session token → recovery guidance, not a raw error.
      if (isAuthDeath(error)) {
        return lifeboatResponse(error);
      }
      return errorResult(
        "tool_call_failed",
        String(error?.message || error),
        "Retry with validated input payload."
      );
    }
  }

  return { listTools, callTool };
}
