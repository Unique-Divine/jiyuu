/**
 * Slack Web API stub for handler tests.
 *
 * Function `slackAPI` in lib/slack-client.js reads credentials from
 * SLACK_TOKEN and SLACK_COOKIE, then POSTs to https://slack.com/api/<method>
 * with the global `fetch`. This stub sets placeholder credentials and swaps
 * `fetch` for one that answers each Slack method from a table, so handler
 * tests run the real client code without network access. Call `restore` in
 * `afterEach` to put the original `fetch` and environment back.
 */

const PLACEHOLDER_TOKEN = "xoxc-test-placeholder"
const PLACEHOLDER_COOKIE = "xoxd-test-placeholder"

/**
 * @param {Record<string, object | ((params: object) => object)>} responses
 *   Slack method name to response body, or to a function of the request
 *   params that returns the body.
 */
export function stubSlackApi(responses) {
  const originalFetch = globalThis.fetch
  const originalToken = process.env.SLACK_TOKEN
  const originalCookie = process.env.SLACK_COOKIE
  const calls = []

  process.env.SLACK_TOKEN = PLACEHOLDER_TOKEN
  process.env.SLACK_COOKIE = PLACEHOLDER_COOKIE

  globalThis.fetch = async (url, init = {}) => {
    const method = String(url).replace("https://slack.com/api/", "")
    const params = parseBody(init)
    calls.push({ method, params })
    const entry = responses[method]
    if (entry === undefined) {
      throw new Error(`slack-api-stub: no response for ${method}`)
    }
    const body = typeof entry === "function" ? entry(params) : entry
    return new Response(JSON.stringify(body), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    })
  }

  return {
    calls,
    restore() {
      globalThis.fetch = originalFetch
      restoreEnv("SLACK_TOKEN", originalToken)
      restoreEnv("SLACK_COOKIE", originalCookie)
    },
  }
}

function parseBody(init) {
  const contentType = init.headers?.["Content-Type"] || ""
  if (contentType.startsWith("application/json")) {
    return JSON.parse(init.body || "{}")
  }
  return Object.fromEntries(new URLSearchParams(init.body || ""))
}

function restoreEnv(key, value) {
  if (value === undefined) {
    delete process.env[key]
  } else {
    process.env[key] = value
  }
}
