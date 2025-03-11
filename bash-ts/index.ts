import type { Subprocess } from "bun"
import * as Bun from "bun"

export interface BashOut {
  stdout: string
  stderr: string
  exitCode: number | null
}

export const bash = async (cmd: string): Promise<BashOut> => {
  const rawOut: Subprocess = Bun.spawn(["bash", "-c", cmd])
  const { stdout, stderr, exitCode } = rawOut
  return {
    stdout: await new Response(
      typeof stdout === "number" ? stdout.toString() : stdout,
    ).text(),
    stderr: await new Response(
      typeof stderr === "number" ? stderr.toString() : stderr,
    ).text(),
    exitCode,
  }
}
