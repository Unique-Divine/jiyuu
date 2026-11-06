import { BunCryptoProvider } from "@mtcute/bun"
import { initSync, SIMD_AVAILABLE } from "@mtcute/wasm"
import wasmPath from "@mtcute/wasm/mtcute.wasm" with { type: "file" }
import simdWasmPath from "@mtcute/wasm/mtcute-simd.wasm" with { type: "file" }

/** Static file imports make Bun embed both lazily loaded dependency assets. */
export class EmbeddedTelegramCryptoProvider extends BunCryptoProvider {
  override async initialize(): Promise<void> {
    const bytes = await Bun.file(SIMD_AVAILABLE ? simdWasmPath : wasmPath).arrayBuffer()
    initSync(bytes)
  }
}

/** The client and readiness check must use this same concrete provider. */
export function createTelegramCryptoProvider(): EmbeddedTelegramCryptoProvider {
  return new EmbeddedTelegramCryptoProvider()
}

export async function verifyRuntimeAssets(): Promise<void> {
  // Read both files so missing fallback assets fail even on a SIMD-capable host.
  for (const path of [wasmPath, simdWasmPath]) {
    const bytes = await Bun.file(path).arrayBuffer()
    if (!bytes.byteLength) throw new Error("Empty Telegram runtime asset")
  }
}
