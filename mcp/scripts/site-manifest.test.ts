import { afterEach, beforeEach, describe, expect, test } from "bun:test"
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import {
  apply,
  assertNoUpstreamBranding,
  buildManifest,
  discoverServers,
  installSection,
  isSecretFileName,
  makePythonToolLister,
  MANIFEST_FILE,
  parseArgs,
  plan,
  readSafeText,
  schemaType,
  summarize,
  toManifestTool,
  type McpTool,
  type Outcome,
  type PythonToolLister,
  type ServerSource,
} from "./site-manifest.ts"

const README = `# Alpha MCP

Intro.

## Install

Run it:

\`\`\`sh
bunx @example/alpha@1.0.0
## not a heading inside a fence
\`\`\`

### Update

Change the pin.

## Install From Source

Clone it.
`

const MCP_INFO = {
  name: "mcp_info",
  description: "Report version.",
  inputSchema: { type: "object", properties: {} },
  annotations: { title: "MCP Info", readOnlyHint: true },
}

const ALPHA_TOOLS = `export const TOOLS = [
  {
    name: "alpha_read",
    description: "Reads a thing.\\n\\nArgs:\\n  id: the id",
    inputSchema: {
      type: "object",
      properties: { id: { type: "string" }, limit: { type: "number" } },
      required: ["id"],
    },
    annotations: { title: "Read", readOnlyHint: true },
  },
  {
    name: "alpha_delete",
    description: "Deletes a thing.",
    inputSchema: { type: "object", properties: { id: { type: "string" } } },
    annotations: { title: "Delete", readOnlyHint: false },
  },
]
export const MCP_INFO_TOOL = ${JSON.stringify(MCP_INFO)}
`

// Lists env names and cwd so tests can check what the server process sees.
const BETA_TOOLS_PY = `import os

async def list_tools():
    return [
        {
            "name": "mcp_info",
            "description": "Report version.",
            "inputSchema": {"type": "object", "properties": {}},
            "annotations": {"title": "MCP Info", "readOnlyHint": True},
        },
        {
            "name": "beta_env",
            "description": " ".join(sorted(os.environ)),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "chat_id": {
                        "anyOf": [{"type": "integer"}, {"type": "string"}]
                    },
                    "account": {
                        "anyOf": [{"type": "string"}, {"type": "null"}],
                        "default": None,
                    },
                },
                "required": ["chat_id"],
            },
            "annotations": {"title": "Env", "destructiveHint": False},
        },
    ]
`

const SECRET_FILES = [".env", "tokens.json", "a.session", "mcp_errors.log"]

let root: string
let mcpDir: string

function write(path: string, text: string): void {
  mkdirSync(join(path, ".."), { recursive: true })
  writeFileSync(path, text)
}

function npmPackage(name: string, site: unknown): string {
  return JSON.stringify({
    name: `@example/${name}`,
    version: "1.0.0",
    description: `${name} server for tests.`,
    license: "MIT",
    engines: { bun: ">=1.4.3" },
    jiyuu: { site },
  })
}

function makeFixture(): void {
  // alpha: npm server with secret-looking files nobody may open.
  const alpha = join(mcpDir, "alpha")
  write(
    join(alpha, "package.json"),
    npmPackage("alpha", {
      tools: ["lib/tools.ts#TOOLS", "lib/tools.ts#MCP_INFO_TOOL"],
      env: [{ name: "ALPHA_TOKEN", required: true, secret: true }],
    }),
  )
  write(join(alpha, "lib/tools.ts"), ALPHA_TOOLS)
  write(join(alpha, "README.md"), README)
  for (const name of SECRET_FILES) {
    write(join(alpha, name), "SECRET=1\n")
    chmodSync(join(alpha, name), 0o000)
  }

  // beta: Python server listed through the real helper.
  const beta = join(mcpDir, "beta")
  write(
    join(beta, "pyproject.toml"),
    `[project]
name = "example-beta"
version = "2.0.0"
description = "beta server for tests."
license = { text = "Apache-2.0" }
requires-python = ">=3.10"

[tool.jiyuu.site]
tools = "beta_tools:list_tools"
env = [{ name = "BETA_API_ID", required = true, secret = true }]

[tool.jiyuu.site.tools-env]
BETA_API_ID = "12345"
`,
  )
  write(join(beta, "beta_tools.py"), BETA_TOOLS_PY)
  write(join(beta, "README.md"), README.replace("Alpha", "Beta"))

  // gamma and delta opt out; scripts has no package manifest.
  write(join(mcpDir, "gamma/package.json"), npmPackage("gamma", false))
  write(
    join(mcpDir, "delta/pyproject.toml"),
    `[project]
name = "delta"
version = "0.1.0"

[tool.jiyuu]
site = false
`,
  )
  write(join(mcpDir, "scripts/helper.ts"), "export {}\n")
}

const python: PythonToolLister = makePythonToolLister(() => ["python3", "-I"])

function options(revision = "abc123") {
  return { mcpDir, revision, listPythonTools: python }
}

function byFolder(outcomes: Outcome[]): Record<string, Outcome["status"]> {
  return Object.fromEntries(outcomes.map((o) => [o.folder, o.status]))
}

function manifestOf(folder: string): Record<string, any> {
  return JSON.parse(readFileSync(join(mcpDir, folder, MANIFEST_FILE), "utf8"))
}

beforeEach(() => {
  root = mkdtempSync(join(tmpdir(), "site-manifest-test-"))
  mcpDir = join(root, "mcp")
  makeFixture()
})

afterEach(() => {
  for (const name of SECRET_FILES) {
    const path = join(mcpDir, "alpha", name)
    if (existsSync(path)) chmodSync(path, 0o600)
  }
  rmSync(root, { recursive: true, force: true })
})

describe("discovery", () => {
  test("finds npm and PyPI servers and honors both opt-out forms", () => {
    const { servers, optedOut } = discoverServers(mcpDir)
    expect(servers.map((s) => [s.folder, s.registry])).toEqual([
      ["alpha", "npm"],
      ["beta", "pypi"],
    ])
    expect(optedOut).toEqual(["delta", "gamma"])
    expect(servers[1]!.runtime).toBe("Python >= 3.10")
    expect(servers[0]!.runtime).toBe("Bun >= 1.4.3")
  })

  test("rejects env entries that carry values", () => {
    write(
      join(mcpDir, "gamma/package.json"),
      npmPackage("gamma", {
        tools: ["x.ts#T"],
        env: [{ name: "GAMMA_TOKEN", value: "leak" }],
      }),
    )
    expect(() => discoverServers(mcpDir)).toThrow(/unknown key value/)
  })
})

describe("never reads secret files", () => {
  test("classifies env, token, session, and log names as secret", () => {
    for (const name of [
      ...SECRET_FILES,
      ".env.local",
      "cookies.txt",
      "creds.pem",
    ]) {
      expect(isSecretFileName(name)).toBe(true)
    }
    for (const name of [
      "package.json",
      "README.md",
      "lib/tools.js",
      "toolList.ts",
    ]) {
      expect(isSecretFileName(name)).toBe(false)
    }
  })

  test("readSafeText refuses secret names and paths outside the server", () => {
    const alpha = join(mcpDir, "alpha")
    expect(() => readSafeText(alpha, ".env")).toThrow(/refusing/)
    expect(() => readSafeText(alpha, "../beta/README.md")).toThrow(/outside/)
  })

  test("generation succeeds with unreadable secret files present", async () => {
    // Opening any of the chmod 000 files would throw EACCES (unless root).
    const outcomes = await plan(options())
    expect(byFolder(outcomes)).toEqual({ alpha: "missing", beta: "missing" })
  })

  test("a tool entry that names a secret file fails before import", async () => {
    write(
      join(mcpDir, "alpha/package.json"),
      npmPackage("alpha", {
        tools: [".env#TOOLS"],
      }),
    )
    await expect(plan(options())).rejects.toThrow(/refusing to read \.env/)
  })

  test("the Python server sees only PATH, HOME, and synthetic env", async () => {
    process.env.SITE_MANIFEST_TEST_SECRET = "do-not-pass"
    try {
      apply(await plan(options()))
    } finally {
      delete process.env.SITE_MANIFEST_TEST_SECRET
    }
    const beta = manifestOf("beta")
    const seen = beta.tools.find((t: any) => t.name === "beta_env").description
    const names = seen.split(" ").filter((n: string) => !n.startsWith("LC_"))
    expect(names.sort()).toEqual(
      ["BETA_API_ID", "HOME", "JIYUU_MCP_NO_UPDATE_CHECK", "PATH"].filter(
        (n) => n !== "HOME" || process.env.HOME !== undefined,
      ),
    )
  })
})

describe("manifest content", () => {
  test("writes the contract shape for npm and PyPI servers", async () => {
    apply(await plan(options("rev1")))
    const alpha = manifestOf("alpha")
    expect(Object.keys(alpha)).toEqual([
      "schemaVersion",
      "folder",
      "slug",
      "title",
      "description",
      "registry",
      "package",
      "version",
      "installCommand",
      "runtime",
      "license",
      "sourceUrl",
      "registryUrl",
      "envVars",
      "toolCount",
      "readOnlyCount",
      "writeCount",
      "tools",
      "installMarkdown",
      "generatedFrom",
    ])
    expect(alpha).toMatchObject({
      schemaVersion: 1,
      slug: "alpha-mcp",
      title: "Alpha MCP",
      registry: "npm",
      installCommand: "bunx @example/alpha@1.0.0",
      registryUrl: "https://www.npmjs.com/package/@example/alpha",
      envVars: [{ name: "ALPHA_TOKEN", required: true, secret: true }],
      toolCount: 3,
      readOnlyCount: 2,
      writeCount: 1,
      generatedFrom: { revision: "rev1" },
    })
    expect(alpha.tools[0]).toEqual({
      name: "alpha_read",
      title: "Read",
      description: "Reads a thing.",
      readOnly: true,
      destructive: false,
      params: [
        { name: "id", type: "string", required: true },
        { name: "limit", type: "number", required: false },
      ],
    })
    // A write tool without destructiveHint takes MCP's default of true.
    expect(alpha.tools[1].destructive).toBe(true)
    expect(alpha.installMarkdown.startsWith("Run it:")).toBe(true)
    expect(alpha.installMarkdown).toContain("### Update")
    expect(alpha.installMarkdown).not.toContain("Clone it.")

    const beta = manifestOf("beta")
    expect(beta).toMatchObject({
      registry: "pypi",
      installCommand: "uvx example-beta@2.0.0",
      registryUrl: "https://pypi.org/project/example-beta/",
      license: "Apache-2.0",
    })
    expect(beta.tools[1].params).toEqual([
      { name: "chat_id", type: "integer | string", required: true },
      { name: "account", type: "string", required: false },
    ])
    expect(beta.tools[1].destructive).toBe(false)
  })

  test("rejects a tool list without mcp_info", () => {
    const source = discoverServers(mcpDir).servers[0] as ServerSource
    const tools: McpTool[] = [{ name: "alpha_read" }]
    expect(() => buildManifest(source, tools, README, "r")).toThrow(/mcp_info/)
  })

  test("rejects upstream branding and pricing", () => {
    const source = discoverServers(mcpDir).servers[0] as ServerSource
    const tools: McpTool[] = [
      MCP_INFO,
      { name: "alpha_ai", description: "Upgrade to Pro $19/mo for unlimited." },
    ]
    expect(() => buildManifest(source, tools, README, "r")).toThrow(/price/)
    const manifest = buildManifest(source, [MCP_INFO], README, "r")
    manifest.description = "Runs on the hosted worker."
    expect(() => assertNoUpstreamBranding(manifest)).toThrow(/hosted/)
  })
})

describe("check mode", () => {
  test("a new revision alone leaves a manifest fresh", async () => {
    apply(await plan(options("rev1")))
    const again = await plan(options("rev2"))
    expect(byFolder(again)).toEqual({ alpha: "fresh", beta: "fresh" })
    apply(again)
    expect(manifestOf("alpha").generatedFrom.revision).toBe("rev1")
  })

  test("a tool change makes the manifest stale", async () => {
    apply(await plan(options()))
    const tools = join(mcpDir, "alpha/lib/tools.ts")
    // A new path avoids Bun's module cache for the edited file.
    write(
      join(mcpDir, "alpha/lib/tools2.ts"),
      readFileSync(tools, "utf8").replace("Reads a thing.", "Reads it."),
    )
    write(
      join(mcpDir, "alpha/package.json"),
      npmPackage("alpha", {
        tools: ["lib/tools2.ts#TOOLS", "lib/tools2.ts#MCP_INFO_TOOL"],
        env: [{ name: "ALPHA_TOKEN", required: true, secret: true }],
      }),
    )
    expect(byFolder(await plan(options()))).toEqual({
      alpha: "stale",
      beta: "fresh",
    })
  })

  test("an opted-out server's manifest is flagged and removed", async () => {
    write(join(mcpDir, "gamma", MANIFEST_FILE), "{}\n")
    const outcomes = await plan(options())
    expect(byFolder(outcomes).gamma).toBe("unwanted")
    apply(outcomes)
    expect(existsSync(join(mcpDir, "gamma", MANIFEST_FILE))).toBe(false)
  })

  test("parseArgs accepts only --check", () => {
    expect(parseArgs([])).toEqual({ check: false })
    expect(parseArgs(["--check"])).toEqual({ check: true })
    expect(() => parseArgs(["--write"])).toThrow(/Usage/)
  })
})

describe("helpers", () => {
  test("summarize keeps the first paragraph on one line", () => {
    expect(summarize("\nGet messages\nfrom a chat.\nArgs:\n  x: y")).toBe(
      "Get messages from a chat.",
    )
    expect(summarize("One.\n\nNote: untrusted.")).toBe("One.")
    expect(summarize(undefined)).toBe("")
  })

  test("schemaType joins union members and drops null", () => {
    expect(schemaType({ type: ["string", "null"] })).toBe("string")
    expect(schemaType({ anyOf: [{ type: "integer" }, { type: "null" }] })).toBe(
      "integer",
    )
    expect(schemaType({})).toBe("any")
  })

  test("installSection ignores headings inside code fences", () => {
    expect(installSection(README)).toContain("## not a heading inside a fence")
    expect(() => installSection("# T\n\n## Usage\n")).toThrow(/Install/)
  })

  test("toManifestTool prefers the top-level title", () => {
    const tool = toManifestTool({
      name: "x",
      title: "Top",
      annotations: { title: "Annotation", readOnlyHint: true },
    })
    expect(tool.title).toBe("Top")
  })
})
