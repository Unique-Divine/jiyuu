#!/usr/bin/env bun

/**
 * File-specific logging functions that produce print statements akin to those
 * made with `go test ./...` logging in Golang.
 *
 * Example: Intended Usage
 * ```
 * const { clog, cerr, clogCmd } = newClog(
 *   import.meta.url.includes("/")
 *     ? import.meta.url.split("/").pop()!
 *     : import.meta.url,
 * )
 * ```
 *
 * clog: Short for [c]onsole [log] wrapper.
 *   This shows the file name and line number when you log.
 * cerr: Short for [c]onsole [err]or wrapper.
 *   This shows the file name and line number when you log.
 * */
export const newClog = (
  fname: string,
): {
  clog: (...args: any[]) => void
  cerr: (...args: any[]) => void
  clogCmd: (cmd: string) => void
} => {
  const {
    clog,
    cerr,
  }: Pick<ReturnType<typeof newClog>, "clog" | "cerr"> = {
    clog: (...args: any[]): void => {
      const e = new Error()
      const stack = e.stack?.split("\n")

      // stack[0] = "Error"
      // stack[1] = at log ...
      // stack[2] = the caller line
      const caller = stack?.[2] ?? ""
      const match = caller.match(/:(\d+):(\d+)\)?$/)

      let location = ""
      if (match) {
        const [, line, _col] = match
        location = `${fname}:[${line}]`
      }

      const inspectedArgs = args.map((arg) =>
        typeof arg === "string"
          ? arg
          : Bun.inspect(arg, { depth: Infinity, colors: false }),
      )
      console.log(`${location}:`, ...inspectedArgs)
    },
    cerr: (...args: any[]): void => {
      const e = new Error()
      const stack = e.stack?.split("\n")

      // stack[0] = "Error"
      // stack[1] = at log ...
      // stack[2] = the caller line
      const caller = stack?.[2] ?? ""
      const match = caller.match(/:(\d+):(\d+)\)?$/)

      let location = ""
      if (match) {
        const [, line, _col] = match
        location = `${fname}:[${line}]`
      }

      const inspectedArgs = args.map((arg) =>
        typeof arg === "string"
          ? arg
          : Bun.inspect(arg, { depth: Infinity, colors: false }),
      )
      console.error(`${location}:`, ...inspectedArgs)
    },
  }

  const clogCmd: ReturnType<typeof newClog>["clogCmd"] = (
    cmd: string,
  ): void => {
    clog(`====== RUN ======\n${cmd}\n`)
  }

  return {
    clog,
    cerr,
    clogCmd,
  }
}
