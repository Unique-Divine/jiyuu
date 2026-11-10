/**
 * Telegram 2FA password storage in `pass` (the standard Unix password
 * manager). Profiles keep only the pass entry name; the value is read with
 * `pass show` when Telegram asks for it.
 */
import { stat } from "node:fs/promises"
import { homedir } from "node:os"
import { join } from "node:path"

/** Environment for `pass`: `PATH`, `PASSWORD_STORE_DIR`, `GNUPGHOME`, ... */
export interface PassEnv {
  [key: string]: string | undefined
}

/**
 * Where a session's 2FA password comes from. `value` holds a password that
 * is already in memory (`TELEGRAM_PASSWORD`, a known credential file, or a
 * profile saved before pass support). `pass` holds only the entry name.
 */
export type TelegramPasswordRef =
  | { kind: "value"; value: string }
  | { kind: "pass"; entry: string }

/** Longest time `pass show` may take, including a GPG agent unlock. */
export const PASS_TIMEOUT_MS = 120_000

/** Default pass entry for a profile's 2FA password. */
export function defaultPasswordPassEntry(profileName: string): string {
  return `tg-sai/${profileName}/telegram-2fa`
}

/** Throws unless `entry` is a relative pass entry name. */
export function validatePassEntry(entry: string): string {
  const trimmed = entry.trim()
  if (
    !trimmed ||
    trimmed !== entry ||
    trimmed.startsWith("-") ||
    trimmed.startsWith("/") ||
    trimmed.endsWith("/") ||
    trimmed.split("/").some((part) => part === "" || part === "." || part === "..") ||
    /[\r\n\0]/.test(trimmed)
  ) {
    throw new Error(`Invalid pass entry name: ${JSON.stringify(entry)}`)
  }
  return trimmed
}

/** Password store directory that `pass` uses for `env`. */
export function passwordStoreDir(env: PassEnv = process.env): string {
  return env.PASSWORD_STORE_DIR ?? join(env.HOME ?? homedir(), ".password-store")
}

/** True when the store has an encrypted file for `entry`. Never decrypts. */
export async function passEntryExists(
  entry: string,
  env: PassEnv = process.env,
): Promise<boolean> {
  try {
    const info = await stat(
      join(passwordStoreDir(env), `${validatePassEntry(entry)}.gpg`),
    )
    return info.isFile()
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return false
    throw error
  }
}

function passBinary(env: PassEnv): string {
  const found = Bun.which("pass", { PATH: env.PATH ?? "" })
  if (!found) {
    throw new Error(
      "pass is not installed or not on PATH. Install pass and run `pass init <gpg-id>`.",
    )
  }
  return found
}

/**
 * Throws unless `pass` is on PATH and the store was initialised with
 * `pass init` (its `.gpg-id` file exists). Never decrypts anything.
 */
export async function assertPassReady(env: PassEnv = process.env): Promise<void> {
  passBinary(env)
  const gpgId = join(passwordStoreDir(env), ".gpg-id")
  try {
    if ((await stat(gpgId)).isFile()) return
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error
  }
  throw new Error(
    `pass store ${passwordStoreDir(env)} is not initialised. Run \`pass init <gpg-id>\` first.`,
  )
}

/** First line of a pass entry, without the newline. */
export function passwordFromPassOutput(output: string, entry: string): string {
  const firstLine = output.split(/\r?\n/, 1)[0] ?? ""
  if (!firstLine) {
    throw new Error(`pass entry ${entry} has an empty first line`)
  }
  return firstLine
}

/**
 * Runs `pass` with `args`. Stderr is dropped so `pass` output never reaches
 * logs; the error names only the step and the entry.
 */
async function runPass(
  args: string[],
  env: PassEnv,
  options: { stdin?: string; timeoutMs?: number; failure: string },
): Promise<string> {
  const child = Bun.spawn([passBinary(env), ...args], {
    env: env as Record<string, string | undefined>,
    stdin: options.stdin === undefined ? "inherit" : "pipe",
    stdout: "pipe",
    stderr: "ignore",
  })
  if (options.stdin !== undefined && child.stdin) {
    child.stdin.write(options.stdin)
    await child.stdin.end()
  }
  let timedOut = false
  const timer = setTimeout(() => {
    timedOut = true
    child.kill("SIGKILL")
  }, options.timeoutMs ?? PASS_TIMEOUT_MS)
  try {
    const [output, exitCode] = await Promise.all([
      new Response(child.stdout).text(),
      child.exited,
    ])
    if (timedOut) {
      throw new Error(`${options.failure}: pass timed out; unlock the GPG agent and retry`)
    }
    if (exitCode !== 0) {
      throw new Error(`${options.failure}; unlock the GPG agent and retry`)
    }
    return output
  } finally {
    clearTimeout(timer)
  }
}

/** Reads the 2FA password (first line) from a pass entry. */
export async function readPassPassword(
  entry: string,
  env: PassEnv = process.env,
): Promise<string> {
  const name = validatePassEntry(entry)
  const output = await runPass(["show", name], env, {
    failure: `pass show failed for entry ${name}`,
  })
  return passwordFromPassOutput(output, name)
}

/** Writes `password` as the only line of a pass entry, replacing it. */
export async function insertPassPassword(
  entry: string,
  password: string,
  env: PassEnv = process.env,
): Promise<void> {
  const name = validatePassEntry(entry)
  assertSingleLinePassword(password)
  await runPass(["insert", "--multiline", "--force", name], env, {
    stdin: `${password}\n`,
    failure: `pass insert failed for entry ${name} (check that \`pass init\` ran)`,
  })
}

/** Throws for an empty or multi-line password. */
export function assertSingleLinePassword(password: string): void {
  if (!password) throw new Error("Telegram 2FA password must not be empty")
  if (/[\r\n]/.test(password)) {
    throw new Error("Telegram 2FA password must be one line")
  }
}

/** The 2FA password for `ref`, read from pass only for a pass entry. */
export async function readTelegramPassword(
  ref: TelegramPasswordRef,
  env: PassEnv = process.env,
): Promise<string> {
  return ref.kind === "value" ? ref.value : readPassPassword(ref.entry, env)
}

/** First line of piped stdin, for `--password-stdin`. */
export function passwordFromStdinText(text: string): string {
  const password = text.split(/\r?\n/, 1)[0] ?? ""
  assertSingleLinePassword(password)
  return password
}

/** Reads the password from piped stdin. */
export async function readPasswordFromStdin(): Promise<string> {
  return passwordFromStdinText(await Bun.stdin.text())
}

/**
 * Prompts on the terminal without echoing input. Requires a TTY; scripts use
 * `--password-stdin`.
 */
export async function promptHiddenPassword(label: string): Promise<string> {
  const stdin = process.stdin
  if (!stdin.isTTY) {
    throw new Error("--password-prompt needs a terminal; pipe the password with --password-stdin")
  }
  process.stderr.write(label)
  stdin.setRawMode(true)
  stdin.resume()
  try {
    const password = await new Promise<string>((resolve, reject) => {
      let value = ""
      const onData = (chunk: Buffer) => {
        for (const char of chunk.toString("utf8")) {
          if (char === "\r" || char === "\n") {
            stdin.off("data", onData)
            resolve(value)
            return
          }
          if (char === "\u0003") {
            stdin.off("data", onData)
            reject(new Error("Password prompt cancelled"))
            return
          }
          if (char === "\u007f" || char === "\b") {
            value = value.slice(0, -1)
            continue
          }
          value += char
        }
      }
      stdin.on("data", onData)
    })
    assertSingleLinePassword(password)
    return password
  } finally {
    stdin.setRawMode(false)
    stdin.pause()
    process.stderr.write("\n")
  }
}
