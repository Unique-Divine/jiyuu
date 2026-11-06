/**
 * Test preload: point every test run at a throwaway TG_SAI_DIR so tests never
 * write profiles, telemetry.db or state.db under the operator's ~/.tg-sai.
 * Tests that need a specific dir still set their own.
 */
import { mkdtempSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

process.env.TG_SAI_DIR = mkdtempSync(join(tmpdir(), "tg-sai-test-"))
