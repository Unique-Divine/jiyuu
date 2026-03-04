import type { Subprocess } from "bun"
import * as Bun from "bun"

/** Output result of a BASH command */
export interface BashOut {
  stdout: string
  stderr: string
  exitCode: number
}

export const bash = async (cmd: string): Promise<BashOut> => {
  const rawOut: Subprocess = Bun.spawn(["bash", "-o", "pipefail", "-c", cmd])
  const { stdout, stderr } = rawOut
  const exitCode = await rawOut.exited
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
