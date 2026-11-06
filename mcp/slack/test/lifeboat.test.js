import { expect, test } from "bun:test"
import {
  AUTH_DEATH_SLACK_CODES,
  authDeathCode,
  buildLifeboatPayload,
  isAuthDeath,
  lifeboatResponse,
  resetLifeboatThrottle,
} from "../lib/lifeboat.js"

const AUTH_CODES = [
  "invalid_auth",
  "not_authed",
  "token_expired",
  "token_revoked",
  "account_inactive",
]

test("every documented auth-death code is classified as AUTH-DEATH", () => {
  for (const code of AUTH_CODES) {
    // As the structured slack_error on a thrown error.
    expect(isAuthDeath({ slack_error: code }), `${code} via slack_error`).toBe(
      true,
    )
    // As a bare `throw new Error(data.error)` where message IS the code.
    expect(isAuthDeath(new Error(code)), `${code} via Error message`).toBe(true)
    // As a raw string.
    expect(isAuthDeath(code), `${code} via string`).toBe(true)
  }
})

test("the code set matches the documented contract exactly", () => {
  expect([...AUTH_DEATH_SLACK_CODES].sort()).toStrictEqual(
    [...AUTH_CODES].sort(),
  )
})

test("non-auth errors are NOT flagged as AUTH-DEATH", () => {
  const notAuth = [
    new Error("channel_not_found"),
    new Error("ratelimited"),
    new Error("not_in_channel"),
    new Error("msg_too_long"),
    new Error("Slack API error"),
    { slack_error: "channel_not_found" },
    "channel_not_found",
    "",
    null,
    undefined,
  ]
  for (const err of notAuth) {
    expect(
      isAuthDeath(err),
      `${JSON.stringify(err)} must not be auth-death`,
    ).toBe(false)
  }
})

test("the wrapped token_auth_failed error is classified as AUTH-DEATH", () => {
  const err = new Error(
    "Slack auth failed (invalid_auth) and auto-heal could not refresh tokens",
  )
  err.code = "token_auth_failed"
  err.slack_error = "invalid_auth"
  expect(isAuthDeath(err)).toBe(true)
})

test("HTTP 401 responses are classified as AUTH-DEATH", () => {
  expect(isAuthDeath({ status: 401 }), "status field").toBe(true)
  expect(isAuthDeath({ httpStatus: 401 }), "httpStatus field").toBe(true)
  expect(
    isAuthDeath(
      new Error("Slack API users.info returned non-JSON (HTTP 401): ..."),
    ),
    "message",
  ).toBe(true)
  expect(isAuthDeath("401"), "bare 401 string").toBe(true)
  expect(isAuthDeath(401), "raw 401 number").toBe(true)
  expect(isAuthDeath({ status: 500 }), "HTTP 500 is not auth-death").toBe(false)
  expect(isAuthDeath(500), "raw 500 number is not auth-death").toBe(false)
})

test("authDeathCode extracts the specific Slack code (or null)", () => {
  expect(authDeathCode({ slack_error: "token_revoked" })).toBe("token_revoked")
  expect(authDeathCode(new Error("account_inactive"))).toBe("account_inactive")
  expect(authDeathCode("not_authed")).toBe("not_authed")
  expect(authDeathCode(new Error("channel_not_found"))).toBe(null)
  expect(authDeathCode({ status: 401 }), "HTTP 401 carries no Slack code").toBe(
    null,
  )
})

test("first message in the window is long-form; subsequent ones are throttled one-liners", () => {
  resetLifeboatThrottle()
  const t0 = 1_000_000

  const first = buildLifeboatPayload(
    { slack_error: "invalid_auth" },
    { now: t0 },
  )
  expect(first.throttled).toBe(false)
  expect(first.code).toBe("slack_auth_expired")
  expect(first.self_fix, "long-form carries self_fix").toBeTruthy()
  expect(
    first.self_fix.includes("npx -y @jtalk22/slack-mcp --setup"),
    "self_fix names the setup command",
  ).toBeTruthy()

  const second = buildLifeboatPayload(
    { slack_error: "invalid_auth" },
    { now: t0 + 60_000 },
  )
  expect(second.throttled, "second call within the hour is throttled").toBe(
    true,
  )
  expect(second.self_fix, "throttled form drops the long self_fix block").toBe(
    undefined,
  )
  expect(
    second.message.length < first.message.length + first.self_fix.length,
    "throttled form is shorter",
  ).toBeTruthy()
  expect(
    second.message.includes("--setup"),
    "throttled form still names the fix",
  ).toBeTruthy()

  // After the throttle window elapses, the long form returns.
  const third = buildLifeboatPayload(
    { slack_error: "invalid_auth" },
    { now: t0 + 60 * 60 * 1000 + 1 },
  )
  expect(third.throttled, "long form returns after the window elapses").toBe(
    false,
  )
  expect(third.self_fix, "long form again carries self_fix").toBeTruthy()
})

test("self-fix is listed FIRST and the hosted option is separate (helpful-first ordering)", () => {
  resetLifeboatThrottle()
  const p = buildLifeboatPayload(
    { slack_error: "token_expired" },
    { now: 5_000_000 },
  )
  expect(p.self_fix, "self_fix present").toBeTruthy()
  expect(p.hosted_option, "hosted_option present by default").toBeTruthy()
  // Ordering contract: the self-fix key precedes hosted_option in the payload.
  const keys = Object.keys(p)
  expect(
    keys.indexOf("self_fix") < keys.indexOf("hosted_option"),
    "self_fix comes before hosted_option",
  ).toBeTruthy()
  expect(
    p.hosted_option.includes("utm_campaign=token_death"),
    "hosted link carries the campaign tag",
  ).toBeTruthy()
  expect(
    p.hosted_option.toLowerCase().includes("free tier"),
    "hosted copy stays honest — names the free tier",
  ).toBeTruthy()
})

test("SLACK_MCP_NO_UPSELL=1 drops the hosted option but keeps the self-fix", () => {
  resetLifeboatThrottle()
  const prev = process.env.SLACK_MCP_NO_UPSELL
  process.env.SLACK_MCP_NO_UPSELL = "1"
  try {
    const long = buildLifeboatPayload(
      { slack_error: "invalid_auth" },
      { now: 7_000_000 },
    )
    expect(long.hosted_option, "no hosted paragraph when opted out").toBe(
      undefined,
    )
    expect(
      long.self_fix,
      "self-fix guidance still shown when opted out",
    ).toBeTruthy()
    expect(
      !long.next_action.toLowerCase().includes("hosted"),
      "next_action drops the hosted mention",
    ).toBeTruthy()

    resetLifeboatThrottle()
    const short = buildLifeboatPayload(
      { slack_error: "invalid_auth" },
      { now: 7_000_000 },
    )
    // (reset above forces long-form; now trigger the throttled form)
    const throttled = buildLifeboatPayload(
      { slack_error: "invalid_auth" },
      { now: 7_000_100 },
    )
    expect(throttled.throttled).toBe(true)
    expect(
      !throttled.message.includes("mcp.revasserlabs.com"),
      "throttled form hides hosted URL when opted out",
    ).toBeTruthy()
    expect(
      short.self_fix,
      "opted-out long form still carries self_fix",
    ).toBeTruthy()
  } finally {
    if (prev === undefined) delete process.env.SLACK_MCP_NO_UPSELL
    else process.env.SLACK_MCP_NO_UPSELL = prev
  }
})

test("lifeboatResponse wraps the payload as an MCP tool error", () => {
  resetLifeboatThrottle()
  const res = lifeboatResponse({ slack_error: "token_revoked" })
  expect(res.isError).toBe(true)
  expect(res.content[0].type).toBe("text")
  const parsed = JSON.parse(res.content[0].text)
  expect(parsed.code).toBe("slack_auth_expired")
  expect(parsed.slack_error).toBe("token_revoked")
})

test("the extraction_error diagnostic is preserved when the error carries one", () => {
  resetLifeboatThrottle()
  const err = new Error("Slack auth failed (invalid_auth)")
  err.code = "token_auth_failed"
  err.slack_error = "invalid_auth"
  err.extraction_error = {
    code: "apple_events_javascript_disabled",
    message: "blocked",
  }
  const p = buildLifeboatPayload(err, { now: 9_000_000 })
  expect(p.extraction_error).toStrictEqual({
    code: "apple_events_javascript_disabled",
    message: "blocked",
  })
})
