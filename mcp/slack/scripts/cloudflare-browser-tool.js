#!/usr/bin/env node

import { writeFileSync } from "node:fs";
import { resolve } from "node:path";

const ACCOUNT_ID = process.env.CLOUDFLARE_ACCOUNT_ID;
const API_TOKEN = process.env.CLOUDFLARE_API_TOKEN || process.env.CF_TERRAFORM_TOKEN;

const ENDPOINTS = new Map([
  ["content", "content"],
  ["markdown", "markdown"],
  ["links", "links"],
  ["snapshot", "snapshot"],
  ["scrape", "scrape"],
  ["json", "json"],
  ["screenshot", "screenshot"],
  ["pdf", "pdf"],
]);

function usage() {
  console.error(`Usage:
  node scripts/cloudflare-browser-tool.js verify
  node scripts/cloudflare-browser-tool.js <mode> <url> [options]

Modes:
  content     Rendered HTML
  markdown    Rendered markdown
  links       Extract links
  snapshot    HTML with inlined resources
  scrape      Extract CSS selectors (use --selectors "h1,.card")
  json        AI-structured extraction (use --schema '{"title":"string"}')
  screenshot  Capture PNG/JPEG (use --out ./page.png)
  pdf         Capture PDF (use --out ./page.pdf)

Options:
  --wait-until <value>   load|domcontentloaded|networkidle0|networkidle2
  --selectors <csv>      CSS selectors for scrape mode
  --schema <json>        JSON schema object for json mode
  --out <path>           Output path for screenshot/pdf
  --full-page            fullPage screenshot (default true)
  --type <png|jpeg>      screenshot type (default png)
`);
}

function getOption(args, name) {
  const idx = args.indexOf(name);
  if (idx === -1 || idx + 1 >= args.length) return null;
  return args[idx + 1];
}

function hasFlag(args, name) {
  return args.includes(name);
}

async function verifyToken() {
  if (!API_TOKEN) {
    throw new Error("Missing API token. Set CLOUDFLARE_API_TOKEN or CF_TERRAFORM_TOKEN.");
  }

  const checks = [];
  if (ACCOUNT_ID) {
    checks.push({
      source: "account",
      url: `https://api.cloudflare.com/client/v4/accounts/${ACCOUNT_ID}/tokens/verify`,
    });
  }
  checks.push({
    source: "user",
    url: "https://api.cloudflare.com/client/v4/user/tokens/verify",
  });

  const failures = [];
  for (const check of checks) {
    const res = await fetch(check.url, {
      headers: {
        Authorization: `Bearer ${API_TOKEN}`,
        "Content-Type": "application/json",
      },
    });

    const raw = await res.text();
    let data = null;
    try {
      data = raw ? JSON.parse(raw) : null;
    } catch {
      failures.push({
        source: check.source,
        status: res.status,
        message: `Non-JSON response: ${raw.slice(0, 200)}`,
      });
      continue;
    }

    if (res.ok && data?.success) {
      return {
        source: check.source,
        result: data.result,
      };
    }

    failures.push({
      source: check.source,
      status: res.status,
      message: JSON.stringify(data?.errors || data || raw),
    });
  }

  throw new Error(`Token verify failed: ${JSON.stringify(failures)}`);
}

async function callBrowserApi(mode, url, options) {
  if (!ACCOUNT_ID) {
    throw new Error("Missing CLOUDFLARE_ACCOUNT_ID.");
  }
  if (!API_TOKEN) {
    throw new Error("Missing API token. Set CLOUDFLARE_API_TOKEN or CF_TERRAFORM_TOKEN.");
  }

  const endpoint = ENDPOINTS.get(mode);
  if (!endpoint) {
    throw new Error(`Unsupported mode: ${mode}`);
  }

  const body = {
    url,
  };

  if (options.waitUntil) {
    body.waitUntil = options.waitUntil;
  }
  if (mode === "scrape" && options.selectors?.length) {
    body.selectors = options.selectors;
  }
  if (mode === "json" && options.schema) {
    body.schema = options.schema;
  }
  if (mode === "screenshot") {
    body.screenshotOptions = {
      type: options.type || "png",
      fullPage: options.fullPage,
    };
  }
  if (mode === "pdf") {
    body.pdfOptions = {
      printBackground: true,
      format: "A4",
    };
  }

  const res = await fetch(
    `https://api.cloudflare.com/client/v4/accounts/${ACCOUNT_ID}/browser-rendering/${endpoint}`,
    {
      method: "POST",
      headers: {
        Authorization: `Bearer ${API_TOKEN}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify(body),
    }
  );

  if (!res.ok) {
    const text = await res.text().catch(() => "");
    throw new Error(`Browser Rendering API failed (${res.status}): ${text || res.statusText}`);
  }

  return res;
}

async function main() {
  const [, , mode, url, ...rest] = process.argv;

  if (!mode) {
    usage();
    process.exit(1);
  }

  if (mode === "verify") {
    const tokenInfo = await verifyToken();
    console.log(
      JSON.stringify(
        {
          status: "ok",
          verify_source: tokenInfo.source,
          token_status: tokenInfo.result?.status || "unknown",
          token_id: tokenInfo.result?.id || null,
          account_id_present: Boolean(ACCOUNT_ID),
        },
        null,
        2
      )
    );
    return;
  }

  if (!url) {
    usage();
    process.exit(1);
  }

  const options = {
    waitUntil: getOption(rest, "--wait-until") || undefined,
    selectors: (getOption(rest, "--selectors") || "")
      .split(",")
      .map((v) => v.trim())
      .filter(Boolean),
    schema: getOption(rest, "--schema") ? JSON.parse(getOption(rest, "--schema")) : undefined,
    out: getOption(rest, "--out") || undefined,
    type: getOption(rest, "--type") || "png",
    fullPage: !hasFlag(rest, "--no-full-page"),
  };

  const res = await callBrowserApi(mode, url, options);

  if (mode === "screenshot" || mode === "pdf") {
    const outputPath = resolve(options.out || (mode === "pdf" ? "./cloudflare-page.pdf" : "./cloudflare-page.png"));
    const buffer = Buffer.from(await res.arrayBuffer());
    writeFileSync(outputPath, buffer);
    console.log(JSON.stringify({ status: "ok", mode, output: outputPath, bytes: buffer.length }, null, 2));
    return;
  }

  const contentType = res.headers.get("content-type") || "";
  if (contentType.includes("application/json")) {
    const json = await res.json();
    console.log(JSON.stringify(json, null, 2));
    return;
  }

  const text = await res.text();
  console.log(text);
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : String(error));
  process.exit(1);
});
