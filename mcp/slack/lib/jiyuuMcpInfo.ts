/**
 * mcp_info and the update notice for Jiyuu's TypeScript MCP servers.
 *
 * Keep this file byte-identical in mcp/discord/src/jiyuuMcpInfo.ts and
 * mcp/slack/lib/jiyuuMcpInfo.ts. Each published package must run on its own,
 * so the module is copied rather than shared through a workspace dependency.
 * mcp/discord/src/jiyuuMcpInfo.test.ts covers its behavior, and both packages'
 * tests fail when the copies drift apart.
 *
 * The server never updates itself: the client config pins an exact version,
 * so what runs is what was reviewed. Two callers read the npm registry's
 * latest version, each with a 2 s timeout:
 *
 * - At startup, in the background, through a 1 h cache. The first successful
 *   tool result after that check carries a one-line notice when the running
 *   version is behind. Failures are silent.
 * - Tool mcp_info reads the registry on every call and rewrites the cache.
 *   When the registry can't be read, it reports the cached answer and marks
 *   it with source "cache".
 *
 * A cached latest version older than the running version is out of date, so
 * both callers ignore it. Set JIYUU_MCP_NO_UPDATE_CHECK=1 to turn the check
 * off.
 */
import { execFile } from "node:child_process";
import { mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve, sep } from "node:path";

export const OPT_OUT_ENV = "JIYUU_MCP_NO_UPDATE_CHECK";
export const CACHE_TTL_MS = 60 * 60 * 1000;
export const FETCH_TIMEOUT_MS = 2000;
/** Only names Jiyuu publishes are checked; upstream names are someone else's. */
export const PUBLISHED_SCOPE = "@uniquedivine/";

type Env = Record<string, string | undefined>;

/** Return the response body for a URL, or throw on any failure. */
export type Fetcher = (url: string, timeoutMs: number) => Promise<string>;

export interface PackageIdentity {
  name: string;
  version: string;
  /** "installed" when the package root sits under node_modules. */
  provenance: "source-checkout" | "installed";
  root: string;
  upstream: unknown;
}

export interface UpdateStatus {
  name: string;
  runningVersion: string;
  latestVersion: string;
  checkedAt: string;
  /** "live" when this call read the registry; "cache" for a cached answer. */
  source: "live" | "cache";
}

export interface GitState {
  commit: string;
  dirty: boolean;
}

export function readPackageIdentity(packageRoot: string): PackageIdentity {
  const root = resolve(packageRoot);
  const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
  return {
    name: String(pkg.name),
    version: String(pkg.version),
    provenance: root.split(sep).includes("node_modules") ? "installed" : "source-checkout",
    root,
    upstream: pkg.jiyuu?.upstream ?? null,
  };
}

export interface GitOptions {
  /** The git executable; tests pass a stand-in. */
  command?: string;
  timeoutMs?: number;
}

/**
 * Commit and dirty flag for a checkout, or null. Runs git asynchronously so a
 * slow `git status` never blocks other tool calls on the event loop.
 */
export async function gitState(root: string, options: GitOptions = {}): Promise<GitState | null> {
  const git = (...args: string[]) =>
    new Promise<string>((resolve, reject) => {
      execFile(
        options.command ?? "git",
        ["-C", root, ...args],
        { encoding: "utf8", timeout: options.timeoutMs ?? 2000 },
        (err, stdout) => (err ? reject(err) : resolve(stdout.trim())),
      );
    });
  try {
    const [commit, status] = await Promise.all([
      git("rev-parse", "HEAD"),
      git("status", "--porcelain", "--", "."),
    ]);
    return { commit, dirty: status !== "" };
  } catch {
    return null;
  }
}

export function isPublishedName(name: string): boolean {
  return name.startsWith(PUBLISHED_SCOPE);
}

export function checkDisabled(env: Env): boolean {
  const value = (env[OPT_OUT_ENV] ?? "").trim().toLowerCase();
  return !["", "0", "false", "no"].includes(value);
}

export function pinCommand(name: string, version: string): string {
  return `bunx ${name}@${version}`;
}

type Identifier = [0, number] | [1, string];
interface ParsedVersion {
  release: number[];
  prerelease: Identifier[];
}

// SemVer 2.0.0 (semver.org), with an optional leading "v".
const VERSION_RE =
  /^v?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-((?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*))?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/;

export function parseVersion(version: string): ParsedVersion | null {
  const match = VERSION_RE.exec(version.trim());
  if (!match) return null;
  const release = [match[1], match[2], match[3]].map(Number);
  // Prerelease identifiers split on dots only (SemVer 11.4).
  const prerelease = match[4] === undefined
    ? []
    : match[4].split(".").map((part): Identifier =>
      /^\d+$/.test(part) ? [0, Number(part)] : [1, part],
    );
  return { release, prerelease };
}

/** SemVer 11.4: numeric < alphanumeric, numbers numerically, text in ASCII order. */
function compareIdentifiers(a: Identifier[], b: Identifier[]): number {
  for (let i = 0; i < Math.min(a.length, b.length); i++) {
    if (a[i][0] !== b[i][0]) return a[i][0] - b[i][0];
    if (a[i][1] !== b[i][1]) return a[i][1] < b[i][1] ? -1 : 1;
  }
  // With an equal prefix, the larger set of identifiers sorts higher.
  return a.length - b.length;
}

/** Whether `candidate` sorts after `running`; unparseable means no. */
export function isNewer(candidate: string, running: string): boolean {
  const c = parseVersion(candidate);
  const r = parseVersion(running);
  if (!c || !r) return false;
  for (let i = 0; i < 3; i++) {
    const diff = c.release[i] - r.release[i];
    if (diff !== 0) return diff > 0;
  }
  if (c.prerelease.length === 0 || r.prerelease.length === 0) {
    // A final release sorts after any prerelease of the same version.
    return c.prerelease.length === 0 && r.prerelease.length > 0;
  }
  return compareIdentifiers(c.prerelease, r.prerelease) > 0;
}

export function updateAvailable(status: UpdateStatus | null): status is UpdateStatus {
  return status !== null && isNewer(status.latestVersion, status.runningVersion);
}

export function latestVersionUrl(name: string): string {
  return `https://registry.npmjs.org/${name}/latest`;
}

export function parseLatestVersion(body: string): string | null {
  try {
    const version = JSON.parse(body)?.version;
    return typeof version === "string" && parseVersion(version) ? version : null;
  } catch {
    return null;
  }
}

export function defaultCacheDir(env: Env): string {
  return join(env.XDG_CACHE_HOME || join(homedir(), ".cache"), "jiyuu-mcp");
}

export function cacheFile(cacheDir: string, name: string): string {
  return join(cacheDir, `${name.replace(/[^A-Za-z0-9._-]/g, "_")}.json`);
}

export interface CacheReadOptions {
  /** A cached latest version older than this one is out of date. */
  runningVersion?: string;
  /** Maximum entry age; null accepts any age. */
  maxAgeMs?: number | null;
}

export function readCache(
  path: string,
  name: string,
  now: Date,
  options: CacheReadOptions = {},
): { latestVersion: string; checkedAt: string } | null {
  try {
    const data = JSON.parse(readFileSync(path, "utf8"));
    if (data?.name !== name || typeof data.latestVersion !== "string") return null;
    const { runningVersion, maxAgeMs = CACHE_TTL_MS } = options;
    if (runningVersion !== undefined && isNewer(runningVersion, data.latestVersion)) return null;
    const age = now.getTime() - Date.parse(data.checkedAt);
    if (!(age >= 0 && (maxAgeMs === null || age < maxAgeMs))) return null;
    return { latestVersion: data.latestVersion, checkedAt: data.checkedAt };
  } catch {
    return null;
  }
}

export function writeCache(path: string, entry: Record<string, string>): void {
  try {
    mkdirSync(dirname(path), { recursive: true });
    const tmp = `${path}.${process.pid}.tmp`;
    writeFileSync(tmp, JSON.stringify(entry) + "\n");
    renameSync(tmp, path);
  } catch {
    // A read-only cache dir only costs a registry read per start.
  }
}

export const fetchText: Fetcher = async (url, timeoutMs) => {
  const response = await fetch(url, {
    headers: { Accept: "application/json", "User-Agent": "jiyuu-mcp-update-check" },
    signal: AbortSignal.timeout(timeoutMs),
  });
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  return await response.text();
};

export interface CheckOptions {
  fetch?: Fetcher;
  env?: Env;
  cacheDir?: string;
  now?: Date;
  timeoutMs?: number;
}

/** Read the registry and rewrite the cache; null on any failure. */
async function readRegistry(
  name: string,
  runningVersion: string,
  path: string,
  now: Date,
  options: CheckOptions,
): Promise<UpdateStatus | null> {
  try {
    const body = await (options.fetch ?? fetchText)(
      latestVersionUrl(name),
      options.timeoutMs ?? FETCH_TIMEOUT_MS,
    );
    const latestVersion = parseLatestVersion(body);
    if (!latestVersion) return null;
    const checkedAt = now.toISOString().replace(/\.\d{3}Z$/, "Z");
    writeCache(path, { name, latestVersion, checkedAt });
    return { name, runningVersion, latestVersion, checkedAt, source: "live" };
  } catch {
    return null;
  }
}

/**
 * The startup check: a fresh cache entry if one exists, else the registry.
 * Returns null when the check is off, the name isn't Jiyuu's, or the registry
 * can't be read. Never throws.
 */
export async function checkForUpdate(
  name: string,
  runningVersion: string,
  options: CheckOptions = {},
): Promise<UpdateStatus | null> {
  const env = options.env ?? process.env;
  if (checkDisabled(env) || !isPublishedName(name)) return null;
  const now = options.now ?? new Date();
  const path = cacheFile(options.cacheDir ?? defaultCacheDir(env), name);
  const cached = readCache(path, name, now, { runningVersion });
  if (cached) return { name, runningVersion, ...cached, source: "cache" };
  return await readRegistry(name, runningVersion, path, now, options);
}

/**
 * The mcp_info check: always the registry, then the cache of any age when the
 * registry can't be read. Returns null when the check is off, the name isn't
 * Jiyuu's, or neither source has an answer. Never throws.
 */
export async function checkLatest(
  name: string,
  runningVersion: string,
  options: CheckOptions = {},
): Promise<UpdateStatus | null> {
  const env = options.env ?? process.env;
  if (checkDisabled(env) || !isPublishedName(name)) return null;
  const now = options.now ?? new Date();
  const path = cacheFile(options.cacheDir ?? defaultCacheDir(env), name);
  const live = await readRegistry(name, runningVersion, path, now, options);
  if (live) return live;
  const cached = readCache(path, name, now, { runningVersion, maxAgeMs: null });
  return cached ? { name, runningVersion, ...cached, source: "cache" } : null;
}

export function formatNotice(status: UpdateStatus): string {
  return (
    `Update available: ${status.name} ${status.latestVersion} ` +
    `(running ${status.runningVersion}). Change the pin to ` +
    `\`${pinCommand(status.name, status.latestVersion)}\` and reload the MCP server.`
  );
}

/** One background check per process; its notice is handed out once. */
export class UpdateNotifier {
  private pending: Promise<void> | null = null;
  private done = false;
  private result: UpdateStatus | null = null;
  private noticeSent = false;

  constructor(private readonly check: () => Promise<UpdateStatus | null>) {}

  start(): void {
    if (this.pending) return;
    this.pending = this.check()
      .catch(() => null)
      .then((status) => {
        this.result = status;
        this.done = true;
      });
  }

  /** The result so far, waiting up to `waitMs` for an in-flight check. */
  async status(waitMs = 0): Promise<UpdateStatus | null> {
    if (waitMs > 0 && this.pending && !this.done) {
      let timer: ReturnType<typeof setTimeout> | undefined;
      await Promise.race([
        this.pending,
        new Promise((resolve) => {
          timer = setTimeout(resolve, waitMs);
        }),
      ]);
      clearTimeout(timer);
    }
    return this.done ? this.result : null;
  }

  takeNotice(): string | null {
    if (!this.done || this.noticeSent || !updateAvailable(this.result)) return null;
    this.noticeSent = true;
    return formatNotice(this.result as UpdateStatus);
  }
}

interface ToolResult {
  content?: unknown[];
  isError?: boolean;
  [key: string]: unknown;
}

/** Append the one-time notice to a successful tool result. */
export function withUpdateNotice<T extends ToolResult>(result: T, notifier: UpdateNotifier): T {
  if (!result || result.isError) return result;
  const notice = notifier.takeNotice();
  if (!notice) return result;
  return { ...result, content: [...(result.content ?? []), { type: "text", text: notice }] };
}

export const MCP_INFO_TOOL = {
  name: "mcp_info",
  description:
    "Report this server's version, install provenance and update status. Call " +
    "this to learn where the server runs from, which version it is, and whether " +
    "a newer release exists. Never read MCP client config files for this: they " +
    "hold secrets. The report holds names and paths only.",
  inputSchema: { type: "object" as const, properties: {}, required: [] as string[] },
  annotations: { title: "MCP Info", readOnlyHint: true },
};

export const MCP_INFO_INSTRUCTIONS =
  "To learn this server's version, where it runs from, or whether an update " +
  "exists, call mcp_info. Never read MCP client config files for this; they " +
  "hold secrets.";

export interface McpInfoInput {
  server: string;
  identity: PackageIdentity;
  git: GitState | null;
  status: UpdateStatus | null;
  env: Env;
  /** Env var prefixes whose set names (never values) the report lists. */
  envPrefixes: string[];
}

export function buildMcpInfo(input: McpInfoInput): Record<string, unknown> {
  const { identity, status, env } = input;
  let notChecked: string | null = null;
  if (checkDisabled(env)) notChecked = `disabled by ${OPT_OUT_ENV}`;
  else if (!isPublishedName(identity.name))
    notChecked = `'${identity.name}' is not a name Jiyuu publishes`;
  else if (!status) notChecked = "registry unreachable and no cached result";
  return {
    server: input.server,
    name: identity.name,
    version: identity.version,
    provenance: {
      kind: identity.provenance,
      packageRoot: identity.root,
      git: input.git,
    },
    upstream: identity.upstream,
    update: {
      latestVersion: status?.latestVersion ?? null,
      updateAvailable: updateAvailable(status),
      checkedAt: status?.checkedAt ?? null,
      // Only an upgrade gets a command; a pin to an older release downgrades.
      pinCommand: updateAvailable(status) ? pinCommand(status.name, status.latestVersion) : null,
      source: status?.source ?? null,
      notChecked,
    },
    runtime: {
      bun: process.versions.bun ?? null,
      node: process.versions.node,
      execPath: process.execPath,
      pid: process.pid,
    },
    envVarsSet: Object.keys(env)
      .filter((key) => env[key] && input.envPrefixes.some((prefix) => key.startsWith(prefix)))
      .sort(),
  };
}

export interface JiyuuMcpInfoChecks {
  /** The startup check behind the one-time notice; default checkForUpdate. */
  startup?: () => Promise<UpdateStatus | null>;
  /** The check each mcp_info call runs; default checkLatest. */
  live?: (env: Env) => Promise<UpdateStatus | null>;
}

/** Wire-up for one server process: identity, notifier and the tool body. */
export class JiyuuMcpInfo {
  readonly identity: PackageIdentity;
  readonly notifier: UpdateNotifier;
  private readonly liveCheck: (env: Env) => Promise<UpdateStatus | null>;

  constructor(
    private readonly server: string,
    packageRoot: string,
    private readonly envPrefixes: string[],
    checks: JiyuuMcpInfoChecks = {},
  ) {
    this.identity = readPackageIdentity(packageRoot);
    const { name, version } = this.identity;
    this.notifier = new UpdateNotifier(checks.startup ?? (() => checkForUpdate(name, version)));
    this.liveCheck = checks.live ?? ((env) => checkLatest(name, version, { env }));
  }

  async callTool(env: Env = process.env) {
    const [status, git] = await Promise.all([
      this.liveCheck(env).catch(() => null),
      this.identity.provenance === "source-checkout" ? gitState(this.identity.root) : null,
    ]);
    const report = buildMcpInfo({
      server: this.server,
      identity: this.identity,
      git,
      status,
      env,
      envPrefixes: this.envPrefixes,
    });
    return { content: [{ type: "text" as const, text: JSON.stringify(report, null, 2) }] };
  }
}
