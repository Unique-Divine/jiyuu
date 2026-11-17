import { afterAll, describe, expect, test } from "bun:test";
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  JiyuuMcpInfo,
  UpdateNotifier,
  buildMcpInfo,
  checkForUpdate,
  fetchText,
  formatNotice,
  gitState,
  isNewer,
  readPackageIdentity,
  withUpdateNotice,
  type Fetcher,
  type PackageIdentity,
  type UpdateStatus,
} from "./jiyuuMcpInfo.js";

const NAME = "@uniquedivine/mcp-test";
const NOW = new Date("2026-11-17T12:00:00Z");
const HOUR = 60 * 60 * 1000;

const tempDirs: string[] = [];
function tempDir(): string {
  const dir = mkdtempSync(join(tmpdir(), "jiyuu-mcp-info-"));
  tempDirs.push(dir);
  return dir;
}
afterAll(() => {
  for (const dir of tempDirs) execFileSync("/bin/rm", ["-rf", dir]);
});

function recordingFetcher(body: string | Error) {
  const urls: string[] = [];
  const fetch: Fetcher = async (url) => {
    urls.push(url);
    if (body instanceof Error) throw body;
    return body;
  };
  return { fetch, urls };
}

const npmBody = (version: string) => JSON.stringify({ name: NAME, version });

describe("isNewer", () => {
  const cases: [string, string, boolean][] = [
    ["1.4.0", "1.3.8", true],
    ["1.3.10", "1.3.9", true],
    ["1.3.8", "1.3.8", false],
    ["1.3.7", "1.3.8", false],
    ["2", "1.9.9", true],
    ["1.3.8", "1.3.8-rc.1", true],
    ["1.3.8-rc.1", "1.3.8", false],
    ["1.3.8-rc.10", "1.3.8-rc.9", true],
    ["1.3.8-beta", "1.3.8-alpha", true],
    ["v1.4.0", "1.3.8", true],
    ["not-a-version", "1.3.8", false],
    ["1.4.0", "local", false],
  ];
  for (const [candidate, running, expected] of cases) {
    test(`${candidate} > ${running} is ${expected}`, () => {
      expect(isNewer(candidate, running)).toBe(expected);
    });
  }
});

describe("checkForUpdate", () => {
  test("cache miss reads the npm registry and writes the cache", async () => {
    const cacheDir = tempDir();
    const { fetch, urls } = recordingFetcher(npmBody("1.4.0"));

    const status = await checkForUpdate(NAME, "1.3.8", { fetch, env: {}, cacheDir, now: NOW });

    expect(urls).toEqual([`https://registry.npmjs.org/${NAME}/latest`]);
    expect(status).toEqual({
      name: NAME,
      runningVersion: "1.3.8",
      latestVersion: "1.4.0",
      checkedAt: "2026-11-17T12:00:00Z",
    });
    const cached = JSON.parse(readFileSync(join(cacheDir, "_uniquedivine_mcp-test.json"), "utf8"));
    expect(cached).toEqual({ name: NAME, latestVersion: "1.4.0", checkedAt: "2026-11-17T12:00:00Z" });
  });

  test("cache hit within a day skips the registry", async () => {
    const cacheDir = tempDir();
    await checkForUpdate(NAME, "1.3.8", {
      fetch: recordingFetcher(npmBody("1.4.0")).fetch, env: {}, cacheDir, now: NOW,
    });
    const { fetch, urls } = recordingFetcher(npmBody("9.9.9"));

    const status = await checkForUpdate(NAME, "1.3.8", {
      fetch, env: {}, cacheDir, now: new Date(NOW.getTime() + 23 * HOUR),
    });

    expect(urls).toEqual([]);
    expect(status?.latestVersion).toBe("1.4.0");
  });

  test("expired cache reads the registry again", async () => {
    const cacheDir = tempDir();
    await checkForUpdate(NAME, "1.3.8", {
      fetch: recordingFetcher(npmBody("1.4.0")).fetch, env: {}, cacheDir, now: NOW,
    });
    const { fetch, urls } = recordingFetcher(npmBody("1.5.0"));

    const status = await checkForUpdate(NAME, "1.3.8", {
      fetch, env: {}, cacheDir, now: new Date(NOW.getTime() + 24 * HOUR),
    });

    expect(urls.length).toBe(1);
    expect(status?.latestVersion).toBe("1.5.0");
  });

  for (const [label, body] of [
    ["network error", new Error("ECONNREFUSED")],
    ["non-JSON body", "<html>rate limited</html>"],
    ["bad version", JSON.stringify({ version: "garbage" })],
  ] as const) {
    test(`${label} is silent and not cached`, async () => {
      const cacheDir = tempDir();
      const status = await checkForUpdate(NAME, "1.3.8", {
        fetch: recordingFetcher(body).fetch, env: {}, cacheDir, now: NOW,
      });
      expect(status).toBeNull();
      expect(existsSync(join(cacheDir, "_uniquedivine_mcp-test.json"))).toBe(false);
    });
  }

  test("env opt-out skips the check", async () => {
    const { fetch, urls } = recordingFetcher(npmBody("1.4.0"));
    const status = await checkForUpdate(NAME, "1.3.8", {
      fetch, env: { JIYUU_MCP_NO_UPDATE_CHECK: "1" }, cacheDir: tempDir(), now: NOW,
    });
    expect(status).toBeNull();
    expect(urls).toEqual([]);
  });

  test("names Jiyuu does not publish are not checked", async () => {
    const { fetch, urls } = recordingFetcher(npmBody("99.0.0"));
    const status = await checkForUpdate("mcp-discord", "1.3.8", {
      fetch, env: {}, cacheDir: tempDir(), now: NOW,
    });
    expect(status).toBeNull();
    expect(urls).toEqual([]);
  });

  test("the default fetcher reads a real local HTTP server", async () => {
    const server = Bun.serve({ port: 0, fetch: () => new Response(npmBody("1.4.0")) });
    try {
      const fetch: Fetcher = (_url, timeoutMs) => fetchText(`http://127.0.0.1:${server.port}/`, timeoutMs);
      const status = await checkForUpdate(NAME, "1.3.8", { fetch, env: {}, cacheDir: tempDir(), now: NOW });
      expect(status?.latestVersion).toBe("1.4.0");
    } finally {
      server.stop(true);
    }
  });

  test("a slow registry times out silently", async () => {
    const server = Bun.serve({
      port: 0,
      fetch: async () => {
        await Bun.sleep(1000);
        return new Response(npmBody("1.4.0"));
      },
    });
    try {
      const fetch: Fetcher = (_url, timeoutMs) => fetchText(`http://127.0.0.1:${server.port}/`, timeoutMs);
      const started = performance.now();
      const status = await checkForUpdate(NAME, "1.3.8", {
        fetch, env: {}, cacheDir: tempDir(), now: NOW, timeoutMs: 200,
      });
      expect(status).toBeNull();
      expect(performance.now() - started).toBeLessThan(900);
    } finally {
      server.stop(true);
    }
  });
});

describe("UpdateNotifier", () => {
  const behind: UpdateStatus = {
    name: NAME, runningVersion: "1.3.8", latestVersion: "1.4.0", checkedAt: "2026-11-17T12:00:00Z",
  };

  test("the notice goes on the first successful result only", async () => {
    const notifier = new UpdateNotifier(async () => behind);
    notifier.start();
    await notifier.status(2000);

    const failed = withUpdateNotice({ content: [{ type: "text", text: "x" }], isError: true }, notifier);
    const first = withUpdateNotice({ content: [{ type: "text", text: "ok" }] }, notifier);
    const second = withUpdateNotice({ content: [{ type: "text", text: "ok" }] }, notifier);

    expect(failed.content).toHaveLength(1);
    expect(first.content).toEqual([
      { type: "text", text: "ok" },
      { type: "text", text: formatNotice(behind) },
    ]);
    expect(formatNotice(behind)).toBe(
      `Update available: ${NAME} 1.4.0 (running 1.3.8). ` +
        `Change the pin to \`bunx ${NAME}@1.4.0\` and reload the MCP server.`,
    );
    expect(second.content).toHaveLength(1);
  });

  test("quiet before the check finishes and when current", async () => {
    let release: () => void = () => {};
    const gate = new Promise<void>((resolve) => (release = resolve));
    const current = { ...behind, runningVersion: "1.4.0" };
    const notifier = new UpdateNotifier(async () => {
      await gate;
      return current;
    });
    notifier.start();
    expect(notifier.takeNotice()).toBeNull();
    release();
    expect(await notifier.status(2000)).toEqual(current);
    expect(notifier.takeNotice()).toBeNull();
  });

  test("a throwing check counts as no result", async () => {
    const notifier = new UpdateNotifier(async () => {
      throw new Error("boom");
    });
    notifier.start();
    expect(await notifier.status(2000)).toBeNull();
    expect(notifier.takeNotice()).toBeNull();
  });
});

describe("mcp_info", () => {
  function packageRoot(pkg: Record<string, unknown>, parent = tempDir()): string {
    mkdirSync(parent, { recursive: true });
    writeFileSync(join(parent, "package.json"), JSON.stringify(pkg));
    return parent;
  }

  test("identity reads name, version and upstream from package.json", () => {
    const root = packageRoot({ name: NAME, version: "1.3.8", jiyuu: { upstream: { version: "1.3.8" } } });
    expect(readPackageIdentity(root)).toEqual({
      name: NAME, version: "1.3.8", provenance: "source-checkout", root, upstream: { version: "1.3.8" },
    });
    const installed = packageRoot({ name: NAME, version: "1.3.8" }, join(tempDir(), "node_modules", "pkg"));
    expect(readPackageIdentity(installed).provenance).toBe("installed");
    expect(readPackageIdentity(installed).upstream).toBeNull();
  });

  test("git state reports commit and dirty flag", () => {
    const dir = tempDir();
    const git = (...args: string[]) => execFileSync("git", ["-C", dir, ...args], { encoding: "utf8" }).trim();
    git("init", "-q");
    writeFileSync(join(dir, "a.txt"), "a");
    git("add", "a.txt");
    git("-c", "user.name=t", "-c", "user.email=t@example.com", "commit", "-qm", "init");
    const commit = git("rev-parse", "HEAD");
    expect(gitState(dir)).toEqual({ commit, dirty: false });
    writeFileSync(join(dir, "a.txt"), "b");
    expect(gitState(dir)).toEqual({ commit, dirty: true });
    expect(gitState(tempDir())).toBeNull();
  });

  const identity: PackageIdentity = {
    name: NAME, version: "1.3.8", provenance: "installed", root: "/x", upstream: { package: "mcp-discord" },
  };
  const secrets = { DISCORD_TOKEN: "fake.discord.token-value", OTHER_SECRET: "nope" };

  test("report carries update fields and env names, never values", () => {
    const status: UpdateStatus = {
      name: NAME, runningVersion: "1.3.8", latestVersion: "1.4.0", checkedAt: "2026-11-17T12:00:00Z",
    };
    const report = buildMcpInfo({
      server: "discord", identity, git: null, status, env: secrets, envPrefixes: ["DISCORD_"],
    });

    expect(report.update).toEqual({
      latestVersion: "1.4.0",
      updateAvailable: true,
      checkedAt: "2026-11-17T12:00:00Z",
      pinCommand: `bunx ${NAME}@1.4.0`,
      notChecked: null,
    });
    expect(report.provenance).toEqual({ kind: "installed", packageRoot: "/x", git: null });
    expect(report.upstream).toEqual({ package: "mcp-discord" });
    expect(report.envVarsSet).toEqual(["DISCORD_TOKEN"]);
    const text = JSON.stringify(report);
    for (const value of Object.values(secrets)) expect(text).not.toContain(value);
  });

  test("report explains why no check ran", () => {
    const build = (name: string, env: Record<string, string>) =>
      (buildMcpInfo({
        server: "discord", identity: { ...identity, name }, git: null, status: null, env, envPrefixes: [],
      }).update as { notChecked: string }).notChecked;
    expect(build("mcp-discord", {})).toBe("'mcp-discord' is not a name Jiyuu publishes");
    expect(build(NAME, { JIYUU_MCP_NO_UPDATE_CHECK: "1" })).toBe("disabled by JIYUU_MCP_NO_UPDATE_CHECK");
    expect(build(NAME, {})).toBe("no registry result yet (offline, pending, or failed)");
  });

  test("this package's tool call reports itself as a source checkout", async () => {
    const info = new JiyuuMcpInfo("discord", join(import.meta.dir, ".."), ["DISCORD_"], async () => null);
    const result = await info.callTool(secrets);
    const report = JSON.parse(result.content[0].text);

    expect(report.name).toBe(JSON.parse(readFileSync(join(import.meta.dir, "..", "package.json"), "utf8")).name);
    expect(report.provenance.kind).toBe("source-checkout");
    expect(report.provenance.git.commit).toMatch(/^[0-9a-f]{40}$/);
    expect(result.content[0].text).not.toContain(secrets.DISCORD_TOKEN);
  });
});

test("the Discord and Slack copies of jiyuuMcpInfo.ts are identical", () => {
  const discord = join(import.meta.dir, "jiyuuMcpInfo.ts");
  const slack = join(import.meta.dir, "..", "..", "slack", "lib", "jiyuuMcpInfo.ts");
  if (!existsSync(slack)) return; // Outside the Jiyuu monorepo, e.g. a packed tarball.
  expect(readFileSync(slack, "utf8")).toBe(readFileSync(discord, "utf8"));
});
