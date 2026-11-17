import { describe, expect, test } from "bun:test"

import { bunfigText, isolatedGlobal, tarballName } from "./smoke-test"

describe("smoke-test isolation", () => {
  test("global installs and bins go under the temporary root", () => {
    const iso = isolatedGlobal("/tmp/x", { HOME: "/home/u" })
    expect(iso).toEqual({
      root: "/tmp/x",
      config: "/tmp/x/bunfig.toml",
      globalDir: "/tmp/x/global",
      binDir: "/tmp/x/bin",
      env: {
        HOME: "/home/u",
        BUN_INSTALL_GLOBAL_DIR: "/tmp/x/global",
        BUN_INSTALL_BIN: "/tmp/x/bin",
      },
    })
  })

  test("overrides win over inherited global-install variables", () => {
    const iso = isolatedGlobal("/tmp/x", {
      BUN_INSTALL_GLOBAL_DIR: "/home/u/.bun/install/global",
    })
    expect(iso.env.BUN_INSTALL_GLOBAL_DIR).toBe("/tmp/x/global")
  })

  test("bunfig redirects globalDir and globalBinDir", () => {
    expect(bunfigText("/tmp/x/global", "/tmp/x/bin")).toBe(
      '[install]\nglobalDir = "/tmp/x/global"\n' +
        'globalBinDir = "/tmp/x/bin"\n',
    )
  })

  test("tarball name matches bun pm pack for a scoped package", () => {
    expect(tarballName("@uniquedivine/tg-sai", "0.3.2")).toBe(
      "uniquedivine-tg-sai-0.3.2.tgz",
    )
  })
})
