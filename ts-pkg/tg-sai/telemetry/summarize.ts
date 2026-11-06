/**
 * Type-based summaries of TL objects for telemetry `req`/`res` columns.
 *
 * The summarizer is an allowlist by value type, not by method: numbers,
 * booleans, null, and `_` constructor names survive; every string and byte
 * buffer becomes its length; secret-looking keys are dropped outright.
 */

export type Summary =
  | number
  | boolean
  | null
  | string
  | Summary[]
  | { [key: string]: Summary }

export interface SummarizeOptions {
  /** Object nesting levels kept below the root. Default 3. */
  maxDepth?: number
  /** Serialized size cap in bytes. Default 2048. */
  maxBytes?: number
}

export const DEFAULT_MAX_DEPTH = 3
export const DEFAULT_MAX_BYTES = 2048
export const ARRAY_ID_LIMIT = 5

/** Keys never recorded, whatever their value (case-insensitive). */
export const SENSITIVE_KEY =
  /accessHash|hash|password|phone|bytes|session|token|srp/i

/**
 * SRP proof fields (`inputCheckPasswordSRP.A`, `.M1`), matched exactly so a
 * lowercase `a` key survives.
 */
export const SENSITIVE_EXACT_KEYS: ReadonlySet<string> = new Set(["A", "M1"])

export function isSensitiveKey(key: string): boolean {
  return SENSITIVE_EXACT_KEYS.has(key) || SENSITIVE_KEY.test(key)
}

/** Keys whose numeric values identify a peer or object in arrays. */
export const ID_KEYS = ["userId", "channelId", "chatId", "id"] as const

/** Methods recorded by name only: their payloads carry credentials. */
export const NAME_ONLY_METHODS: ReadonlySet<string> = new Set([
  "account.getPassword",
  "account.checkPassword",
  "account.getPasswordSettings",
  "account.updatePasswordSettings",
])

/** Methods whose `password` field is stripped before summarizing. */
export const PASSWORD_FIELD_METHODS: ReadonlySet<string> = new Set([
  "channels.editCreator",
  "messages.getBotCallbackAnswer",
  "payments.sendStarsForm",
])

type Plain = Record<string, unknown>

function isPlainObject(value: unknown): value is Plain {
  return typeof value === "object" && value !== null
}

function isBytes(value: unknown): value is ArrayBufferView | ArrayBuffer {
  return ArrayBuffer.isView(value) || value instanceof ArrayBuffer
}

/** Long.js-style 64-bit values (`{low, high, unsigned}`). */
function isLongLike(value: Plain): boolean {
  return (
    typeof value.low === "number" &&
    typeof value.high === "number" &&
    typeof value.unsigned === "boolean" &&
    typeof value.toString === "function"
  )
}

function byteLength(value: ArrayBufferView | ArrayBuffer): number {
  return value.byteLength
}

/** Numeric id from a Long, bigint, or number; null otherwise. */
function numericId(value: unknown): number | string | null {
  if (typeof value === "number" && Number.isFinite(value)) return value
  if (typeof value === "bigint") return value.toString()
  if (isPlainObject(value) && isLongLike(value)) return String(value)
  return null
}

/**
 * Collects up to `limit` peer/object ids from array elements, looking at the
 * element itself and one level down (e.g. `inputUser.userId`,
 * `inputPeerChannel.channelId`).
 */
export function collectIds(
  items: readonly unknown[],
  limit: number = ARRAY_ID_LIMIT,
): (number | string)[] {
  const ids: (number | string)[] = []
  const visit = (value: unknown, depth: number): void => {
    if (ids.length >= limit || !isPlainObject(value) || isBytes(value)) return
    for (const key of ID_KEYS) {
      const id = numericId(value[key])
      if (id !== null) {
        ids.push(id)
        return
      }
    }
    if (depth > 0) {
      for (const [key, child] of Object.entries(value)) {
        if (isSensitiveKey(key)) continue
        visit(child, depth - 1)
        if (ids.length >= limit) return
      }
    }
  }
  for (const item of items) {
    const id = numericId(item)
    if (id !== null) ids.push(id)
    else visit(item, 1)
    if (ids.length >= limit) break
  }
  return ids
}

function elementType(value: unknown): string {
  if (value === null) return "null"
  if (Array.isArray(value)) return "array"
  if (isBytes(value)) return "bytes"
  if (isPlainObject(value)) {
    return typeof value._ === "string" ? value._ : "object"
  }
  return typeof value
}

function summarizeAt(value: unknown, depth: number, maxDepth: number): Summary {
  if (value === null || value === undefined) return null
  if (typeof value === "number") return Number.isFinite(value) ? value : null
  if (typeof value === "boolean") return value
  if (typeof value === "bigint") return value.toString()
  if (typeof value === "string") return { len: value.length }
  if (typeof value === "function" || typeof value === "symbol") return null
  if (isBytes(value)) return { len: byteLength(value) }
  if (Array.isArray(value)) {
    const out: { [key: string]: Summary } = { n: value.length }
    if (value.length > 0) {
      out.ids = collectIds(value)
      out.of = elementType(value[0])
    }
    return out
  }
  if (!isPlainObject(value)) return null
  if (isLongLike(value)) return String(value)
  const out: { [key: string]: Summary } = {}
  if (typeof value._ === "string") out._ = value._
  if (depth >= maxDepth) {
    if (out._ === undefined) out.truncated = "depth"
    return out
  }
  for (const [key, child] of Object.entries(value)) {
    if (key === "_" || isSensitiveKey(key)) continue
    if (child === undefined) continue
    out[key] = summarizeAt(child, depth + 1, maxDepth)
  }
  return out
}

function serializedBytes(summary: Summary): number {
  return new TextEncoder().encode(JSON.stringify(summary)).length
}

/**
 * Summarizes any TL-shaped value. If the result exceeds `maxBytes`, retries
 * at shallower depths down to 1, then falls back to a `{_, truncated, bytes}` marker
 * (always valid JSON for the `json_valid` column checks).
 */
export function summarizeTl(
  value: unknown,
  opts: SummarizeOptions = {},
): Summary {
  const maxDepth = opts.maxDepth ?? DEFAULT_MAX_DEPTH
  const maxBytes = opts.maxBytes ?? DEFAULT_MAX_BYTES
  let bytes = 0
  for (let depth = maxDepth; depth >= Math.min(1, maxDepth); depth -= 1) {
    const summary = summarizeAt(value, 0, depth)
    bytes = serializedBytes(summary)
    if (bytes <= maxBytes) return summary
  }
  const marker: { [key: string]: Summary } = { truncated: "size", bytes }
  if (isPlainObject(value) && typeof value._ === "string") {
    return { _: value._, ...marker }
  }
  return marker
}

/** True for methods whose request is recorded by name only. */
export function isNameOnlyMethod(method: string): boolean {
  return method.startsWith("auth.") || NAME_ONLY_METHODS.has(method)
}

/**
 * Summarizes an outgoing request. Credential-bearing methods record only
 * `{_}`; methods with a `password` argument (e.g. `channels.editCreator`)
 * keep their peer ids but never the password object.
 */
export function summarizeRequest(
  method: string,
  request: unknown,
  opts: SummarizeOptions = {},
): Summary {
  if (isNameOnlyMethod(method)) return { _: method }
  if (PASSWORD_FIELD_METHODS.has(method) && isPlainObject(request)) {
    const { password: _password, ...rest } = request
    return summarizeTl(rest, opts)
  }
  return summarizeTl(request, opts)
}
