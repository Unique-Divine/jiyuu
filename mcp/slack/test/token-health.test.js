import { afterEach, expect, test } from "bun:test"
import { checkTokenHealth, manualRefreshAdvice, slackAPI } from "../lib/slack-client.js"
import { stubSlackApi } from "./helpers/slack-api-stub.js"

let stub
afterEach(() => {
  stub?.restore()
  stub = undefined
})

const DAY_MS = 24 * 60 * 60 * 1000
const silent = { error: () => {} }

function creds({ ageDays, lastAutoHealError = null, stuckDaysAgo = null }) {
  const now = Date.now()
  return {
    token: "xoxc-test-placeholder",
    cookie: "xoxd-test-placeholder",
    source: "file",
    updatedAt: new Date(now - ageDays * DAY_MS).toISOString(),
    lastAutoHealAttempt: stuckDaysAgo === null ? null : new Date(now - DAY_MS).toISOString(),
    lastAutoHealError,
    stuckSince: stuckDaysAgo === null ? null : new Date(now - stuckDaysAgo * DAY_MS).toISOString(),
  }
}

test("Linux: old unsupported_platform telemetry isn't reported as failing or stuck", async () => {
  const health = await checkTokenHealth(silent, {
    autoHealAvailable: false,
    platformName: "linux",
    creds: creds({ ageDays: 11, lastAutoHealError: "unsupported_platform", stuckDaysAgo: 3 }),
  })

  expect(health).toMatchObject({
    warning: true,
    critical: false,
    stuck: false,
    age_state: "warning",
    auto_heal_available: false,
    last_auto_heal_error: null,
    stuck_since: null,
  })
  expect(health.message).toContain("Token is getting old.")
  expect(health.message).toContain("Auto-refresh only runs on macOS, not linux")
  expect(health.message).toContain("--setup")
  expect(health.message).not.toContain("Apple Events")
  expect(health.message).not.toContain("failing")
})

test("Linux: a critical token keeps the expiry warning with manual advice", async () => {
  const health = await checkTokenHealth(silent, {
    autoHealAvailable: false,
    platformName: "linux",
    creds: creds({ ageDays: 14 }),
  })

  expect(health.critical).toBe(true)
  expect(health.message).toContain("Token may expire soon.")
  expect(health.message).toContain(manualRefreshAdvice("linux"))
})

test("macOS: real auto-heal failures still report stuck with the Apple Events fix", async () => {
  // A 5-day-old token is below the refresh threshold, so no Chrome
  // extraction runs during the test.
  const health = await checkTokenHealth(silent, {
    autoHealAvailable: true,
    platformName: "darwin",
    creds: creds({ ageDays: 5, lastAutoHealError: "apple_events_javascript_disabled", stuckDaysAgo: 2 }),
  })

  expect(health.stuck).toBe(true)
  expect(health.age_state).toBe("stuck")
  expect(health.auto_heal_available).toBe(true)
  expect(health.message).toContain("Auto-heal has been failing")
  expect(health.message).toContain("Apple Events")
})

test("macOS: a healthy token says so", async () => {
  const health = await checkTokenHealth(silent, {
    autoHealAvailable: true,
    platformName: "darwin",
    creds: creds({ ageDays: 1 }),
  })
  expect(health.message).toBe("Token is healthy")
})

test.skipIf(process.platform === "darwin")(
  "non-macOS: invalid_auth gives manual-refresh advice, not Apple Events",
  async () => {
    stub = stubSlackApi({ "users.info": { ok: false, error: "invalid_auth" } })

    let error
    try {
      await slackAPI("users.info", { user: "U1" })
    } catch (e) {
      error = e
    }

    expect(error?.code).toBe("token_auth_failed")
    expect(error.slack_error).toBe("invalid_auth")
    expect(error.next_action).toContain("--setup")
    expect(error.next_action).not.toContain("Apple Events")
    expect(stub.calls.map((c) => c.method)).toStrictEqual(["users.info"])
  },
)
