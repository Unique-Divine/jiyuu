import { bash } from "@uniquedivine/bash"
import { expect, test } from "bun:test"

test("getting started", async () => {
  const out = await bash(`echo hello`)
  expect(out.stdout).toBe("hello\n")
})
