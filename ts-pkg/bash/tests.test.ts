import { expect, test } from "bun:test"
import { bash } from "@uniquedivine/bash"

test("getting started", async () => {
  const out = await bash(`echo hello`)
  expect(out.stdout).toBe("hello\n")
})
