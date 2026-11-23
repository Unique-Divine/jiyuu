import { expect, test } from "bun:test"
import { readFileSync } from "node:fs"
import { fileURLToPath } from "node:url"

const graphQlGuide = readFileSync(
  new URL("../ai-skills/sai-deals/references/graphql.md", import.meta.url),
  "utf8",
)
const fixturePath = fileURLToPath(
  new URL(
    "../ai-skills/sai-deals/examples/atlas-fixture.json",
    import.meta.url,
  ),
)

function example(heading: string, index = 0): string {
  const section = graphQlGuide.split(`## ${heading}\n`)[1]?.split("\n## ")[0]
  const code = [...(section ?? "").matchAll(/```bash\n([\s\S]*?)\n```/g)][
    index
  ]?.[1]
  if (!code) throw new Error(`Missing Bash example in ${heading}`)
  return code
}

const mockCurl = String.raw`
curl() {
  [ "$1" = --disable ] || return 2
  IFS= read -r header
  [ "$header" = "Authorization: Bearer fixture-key" ] || return 22
  body=''
  endpoint=''
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --data-binary) body="$2"; shift ;;
      https://ops-api.sai.fun/graphql) endpoint="$1" ;;
    esac
    shift
  done
  [ "$endpoint" = https://ops-api.sai.fun/graphql ] || return 2
  jq -ce --argjson request "$body" '
    . as $fixture
    | $request.query as $query
    | $request.variables as $vars
    | if $query == "query { credential }" then
        {data: {credential: $fixture.credential}}
      elif ($query | startswith("query DealBrief")) and ($query | contains("payments,completions,actions")) then
        {data: {view: $fixture.views[$vars.selector]}}
      elif $vars.deal != "deal-example-atlas" then
        {errors: [{message: "Synthetic access error"}]}
      elif ($query | startswith("query CommsPage")) and ($query | contains("endCursor hasNextPage")) then
        {data: {commsRecords: $fixture.commsPages[$vars.after // ""]}}
      elif ($query | startswith("query CommsDetail")) and ($query | contains("deliveryEvidenceTruncated")) then
        {data: {commsRecord: $fixture.commsDetails[$vars.id]}}
      else
        {errors: [{message: "Unrecognized synthetic query"}]}
      end
  ' "$ATLAS_FIXTURE"
}
`

function run(commands: string, credential = "fixture-key") {
  return Bun.spawnSync(
    ["bash", "-c", `set -e -o pipefail\n${mockCurl}\n${commands}`],
    {
      env: {
        PATH: process.env.PATH,
        SAI_DEAL_DESK_TOKEN: credential,
        ATLAS_FIXTURE: fixturePath,
      },
    },
  )
}

const auth = example("Check the Credential")
const view = example("Read a Deal Brief")
const commsPage = example("Read Communications")
const commsDetail = example("Read Communications", 1)
const suppressOutput = (code: string) => `{\n${code}\n} >/dev/null`

test("documented reads produce one brief across two communication pages", () => {
  const result = run(
    [
      suppressOutput(auth),
      suppressOutput(view),
      suppressOutput(commsPage),
      `first_page="$page"`,
      "next=\"$(printf '%s\\n' \"$page\" | jq -er '.commsRecords.pageInfo | select(.hasNextPage) | .endCursor')\"",
      suppressOutput(commsPage.replace("after=''", 'after="$next"')),
      `paid_citation="$(printf '%s\n' "$page" | jq -er '.commsRecords.nodes[] | select(.paymentIds == ["payment-atlas-1"]) | .id')"`,
      `detail="$(\n${commsDetail.replace("comms_id='completed_note:comms-atlas-paid'", 'comms_id="$paid_citation"')}\n)"`,
      `uncertain="$(\n${commsDetail.replace("completed_note:comms-atlas-paid", "workflow:comms-atlas-check")}\n)"`,
      `jq -n --argjson view "$view" --argjson page "$page" --argjson first "$first_page" --argjson detail "$detail" --argjson uncertain "$uncertain" '{
      deal: $view.view.deals[0].deal.id,
      payment: ($view.view.deals[0].payments[0] | {id, form, amountUsdc}),
      action: $view.view.deals[0].actions[0].id,
      cutoff: $view.view.deals[0].latestEvaluations[0].measurement.chainBlockTime,
      lastPage: ($page.commsRecords.pageInfo.hasNextPage | not),
      link: $detail.paymentIds[0],
      deliveredAt: $detail.deliveryEvidence[0].deliveredAt,
      undated: ($first.commsRecords.nodes[] | select(.sourceId == "comms-atlas-note") | .deliveredAt),
      uncertain: $uncertain.deliveryEvidence[0].status
    }'`,
    ].join("\n"),
  )

  expect(result.exitCode).toBe(0)
  expect(JSON.parse(result.stdout.toString())).toEqual({
    deal: "deal-example-atlas",
    payment: {
      id: "payment-atlas-1",
      form: "trading_credit",
      amountUsdc: "250.00",
    },
    action: "action-atlas-setup",
    cutoff: "2026-11-20T00:00:00Z",
    lastPage: true,
    link: "payment-atlas-1",
    deliveredAt: "2026-11-18T16:12:00Z",
    undated: null,
    uncertain: "UNCERTAIN",
  })
})

test("the same deal lookup preserves an ambiguous match", () => {
  const wallet = "0x" + "11".repeat(20)
  const result = run(
    [suppressOutput(auth), view.replace("EXAMPLE-ATLAS", wallet)].join("\n"),
  )
  expect(result.exitCode).toBe(0)
  expect(JSON.parse(result.stdout.toString())).toEqual({
    deals: ["deal-example-atlas", "deal-example-boreal"],
    errors: [],
  })
})

test("invalid credentials and GraphQL errors stop the examples", () => {
  const rejected = run(auth, "bad-fixture-key")
  expect(rejected.exitCode).not.toBe(0)
  expect(rejected.stderr.toString()).not.toContain("bad-fixture-key")
  const badDeal = run(
    [
      suppressOutput(auth),
      commsPage.replace("deal-example-atlas", "other-deal"),
    ].join("\n"),
  )
  expect(badDeal.exitCode).not.toBe(0)
  expect(badDeal.stderr.toString()).toContain("Deal Desk GraphQL query failed")
  expect(badDeal.stdout.toString()).toBe("")
})
