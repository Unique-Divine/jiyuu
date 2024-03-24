/*!
 * lib/generate.ts
 *
 * markdown-toc <https://github.com/Unique-Divine/markdown-toc>
 *
 * Forked from legacy code:
 * - markdown-toc <https://github.com/jonschlinkert/markdown-toc>
 *
 * Released under the MIT License.
 */

import * as querystring from "querystring" // NOTE querystring API is deprecated
import markdownLink from "markdown-link"
import listitem from "list-item"
import { RemarkablePlus } from "./remarkable"
import { slugify, SlugifyOptions, getTitle } from "./options"
import { insert } from "./insert"

export { insert }

export { slugify }

interface GenerateOptions extends SlugifyOptions {
  firsth1?: boolean
  maxdepth?: number
  linkify?: boolean | LinkifyFn
  num?: number
  strip?: boolean | string[] | StripFn
  titleize?: boolean | TitleizeFn
  // Add other possible options here...
  hLevel?: number
  bullets?: string[]
  chars?: string[]
  filter?: FilterFn
  append?: string
  highest?: number
}

type LinkifyFn = (
  tok: Token,
  text: string,
  slug: string,
  options: GenerateOptions,
) => string

type StripFn = (str: string, opts?: GenerateOptions) => string

type TitleizeFn = (str: string, options?: GenerateOptions) => string

/** Returns a boolean indicating whether to filter the item */
type FilterFn = (content: string, ele: Token, arr: Token[]) => boolean

export interface Token {
  content: string
  type?: string
  lvl: number
  hLevel: number
  i?: number
  seen?: number
  slug?: string
  children?: Token[]
  lines: number[]
}

export interface JsonToken {
  content: string
  slug: string
  lvl: number
  i: number
  seen: number
}

export function generate(options?: GenerateOptions): any {
  const opts = { ...{ firsth1: true, maxdepth: 6 }, ...options }
  const stripFirst = opts.firsth1 === false
  if (opts.linkify === undefined) opts.linkify = true

  return function (md: RemarkablePlus) {
    md.renderer.render = function (tokens: Token[]) {
      const copiedTokens = [...tokens]
      const seen: { [key: string]: number } = {}
      let tocstart = -1
      const arr: Token[] = []
      const res: {
        json?: JsonToken[]
        highest?: number
        tokens?: Token[]
        content?: string
      } = {}

      for (let i = 0; i < copiedTokens.length; i++) {
        const token = copiedTokens[i]
        if (/<!--[ \t]*toc[ \t]*-->/.test(token.content)) {
          tocstart = token.lines[1]
        }

        if (token.type === "heading_open") {
          copiedTokens[i + 1].lvl = copiedTokens[i].hLevel
          copiedTokens[i + 1].i = i
          arr.push(copiedTokens[i + 1])
        }
      }

      let result: Token[] = []
      res.json = []

      for (const tok of arr) {
        if (tok.lines && tok.lines[0] > tocstart) {
          let val = tok.content
          if (seen[val] === undefined) seen[val] = 0
          if (tok.children && tok.children[0].type === "link_open") {
            if (tok.children[1].type === "text") {
              val = tok.children[1].content
            }
          }

          tok.seen = seen[val]
          tok.slug = slugify(val, opts)
          const jsonToken: JsonToken = {
            content: tok.content,
            slug: seen[val] === 0 ? tok.slug : `${tok.slug}-${seen[val]}`,
            lvl: tok.lvl,
            i: tok.i,
            seen: tok.seen,
          }
          res.json.push(jsonToken)

          if (opts.linkify) {
            const tokLinkified = linkify(tok, opts)
            result.push(tokLinkified)
          } else {
            result.push(tok)
          }

          seen[val] += 1
        }
      }

      opts.highest = highest(result)
      res.highest = opts.highest
      res.tokens = copiedTokens

      if (stripFirst) result = result.slice(1)
      const { out, bList } = bullets(result, opts)
      res.content = out
      if (opts.append) res.content += opts.append
      return res
    }
  }
}

/** Func that creates a single markdown list item for the Toc. */
type ListItemFn = (level: number, content: string) => string

const isListItemFn = (fn: any): fn is ListItemFn => {
  if (typeof fn !== "function") {
    return false // wrong type of object
  } else if (fn.length !== 2) {
    return false // wrong number of args
  }

  try {
    const tempResult = fn(1, "test")
    return typeof tempResult === "string"
  } catch (_error) {
    // If calling the func throws an error, it's not a ListItemFn
    return false
  }
}

const newListitemFn = (
  opts: GenerateOptions,
  fn: FilterFn | null,
): ListItemFn => {
  const liFn = listitem(opts, fn as Function)
  if (isListItemFn(liFn)) return liFn
  throw Error(`list item fn has the wrong type ${JSON.stringify(liFn)}`)
}

/**
 * Render markdown list bullets
 *
 * @param  {Array} `arr` Array of listitem objects
 * @param  {GenerateOptions} `options`
 * @return {String}
 */
export function bullets(
  arr: Token[],
  options?: GenerateOptions,
): { out: string; bList: BulletList } {
  const opts = {
    indent: "  ",
    ...options,
  }

  opts.chars = opts.chars || opts.bullets || ["-", "*", "+"]

  const fn = typeof opts.filter === "function" ? opts.filter : null
  const li: ListItemFn = newListitemFn(opts, fn)

  const includeFirstH1 = opts.firsth1 === true
  return new BulletsParser(arr).bullets(includeFirstH1, fn, li, opts)
}

interface IBulletState {
  firstIsH1: null | boolean
  numH1s: number
  noTokens: boolean
  soloToken: boolean
  tokens: Token[]
}

type BulletList = { bullet: string; lvl: number; content: string }[]

export class BulletsParser implements IBulletState {
  firstIsH1: null | boolean
  numH1s: number
  noTokens: boolean
  soloToken: boolean
  tokens: Token[]
  hs: { lvl: number; content: string }[]

  constructor(tokens: Token[]) {
    this.noTokens = tokens.length === 0
    this.soloToken = tokens.length === 1
    this.firstIsH1 = null
    this.numH1s = 0
    this.tokens = tokens
    this.hs = []
    if (tokens.length < 2) return
    this.parseTokens()
  }

  parseTokens = (): void => {
    console.debug("DEBUG parseTokens: %o", this.tokens)
    this.tokens.forEach((token, idx) => {
      const isH1 = token.lvl === 1
      if (isH1) this.numH1s += 1
      if (idx === 0) {
        this.firstIsH1 = isH1
      }
    })
  }

  otherH1sPresent = (): boolean => {
    if (this.numH1s === 0) return false
    else if (this.firstIsH1 && this.numH1s > 1) return true
    else if (!this.firstIsH1 && this.numH1s > 0) return true
    return false
  }

  bulletList = (
    includeFirstH1: boolean,
    fn: FilterFn,
    li: ListItemFn,
    opts?: GenerateOptions,
  ): BulletList => {
    const indent: boolean = this.indent(includeFirstH1)
    const unindent: number = !this.indent(includeFirstH1) ? 1 : 0
    console.debug("DEBUG obj: %o", { includeFirstH1, unindent, indent })
    const bList: BulletList = []
    this.tokens.forEach((ele) => {
      ele.lvl -= unindent

      if (fn && !fn(ele.content, ele, this.tokens)) {
        return
      }

      const maxDepth = opts.maxdepth ?? 3
      // console.debug("DEBUG obj: %o", { opts, maxDepth, tokens: this.tokens })
      if (ele.lvl > maxDepth) {
        return
      }

      const lvl = ele.lvl - opts.highest
      const { content } = ele
      bList.push({ bullet: li(lvl, content), lvl, content })
    })
    return bList
  }

  bullets = (
    includeFirstH1: boolean,
    fn: FilterFn,
    li: ListItemFn,
    opts?: GenerateOptions,
  ): { out: string; bList: BulletList } => {
    const bList = this.bulletList(includeFirstH1, fn, li, opts)
    return {
      out: bList.map((item) => item.bullet).join("\n"),
      bList,
    }
  }

  // assume arr.length >=  2   -> 2 headings
  //
  // case: h1, h2, ..., h1, ... -> indent
  // "first is h1" AND "other h1s present"
  //
  // case: h2, h2, ... -> unindent
  // NOT "first is h1" AND NOT "other h1s present"
  //
  // case: h1, h2, ... -> if includeFirstH1 ? indent : unindent
  // "first is h1" AND NOT "other h1s present"
  //
  // case: h2, ..., h1, ...  -> indent
  // NOT "first is h1" AND "other h1s present"
  //
  // case: arr.length = 0   -> error
  // case: arr.length = 1   -> unindent
  indent = (includeFirstH1: boolean): boolean => {
    const { firstIsH1 } = this
    const otherH1sPresent = this.otherH1sPresent()

    console.debug("DEBUG obj: %o", { otherH1sPresent, firstIsH1 })

    if (firstIsH1 && otherH1sPresent) return true
    else if (firstIsH1 && !otherH1sPresent) return includeFirstH1 ? true : false
    else if (!firstIsH1 && otherH1sPresent) return true
    else if (!firstIsH1 && !otherH1sPresent) return true
    else return false
    // if (this.soloToken) return false
    // if (this.noTokens) return false
  }
}

/**
 * Get the highest heading level in the array, so
 * we can un-indent the proper number of levels.
 *
 * @param {Array} `arr` Array of tokens
 * @return {Number} Highest level
 */
export function highest(arr: Token[]): number {
  const sorted = arr.slice().sort((a, b) => a.lvl - b.lvl)

  return sorted.length ? sorted[0].lvl : 0
}

/**
 * Turn headings into anchors
 */
export function linkify(tok: Token, options?: GenerateOptions): Token {
  const opts = { ...options }
  if (tok && tok.content) {
    opts.num = tok.seen
    const text = titleize(tok.content, opts)
    const slug = slugify(tok.content, opts)
    // const escapedSlug = querystringEscape(slug)
    const escapedSlug = (querystring as any).escape(slug)

    // console.debug("DEBUG linkfiy: %o", { escapedSlug, opts, tok, text, slug, })
    let linkifyFunc: LinkifyFn = (_tok, text, escapedSlug, _opts) =>
      markdownLink(text, `#${escapedSlug}`)
    if (opts && typeof opts.linkify === "function") {
      linkifyFunc = opts.linkify
    }
    tok.content = linkifyFunc(tok, text, escapedSlug, opts)
  }
  return tok
}
/**
 * Titleize the title part of a markdown link.
 */
export function titleize(str: string, opts?: GenerateOptions): string {
  if (opts && opts.strip) {
    return strip(str, opts)
  }

  if (opts && opts.titleize === false) return str

  if (opts && typeof opts.titleize === "function") {
    return opts.titleize(str, opts)
  }

  str = getTitle(str)

  str = str.split(/<\/?[^>]+>/).join("")

  str = str.split(/[ \t]+/).join(" ")

  return str.trim()
}

/**
 * Optionally strip specified words from heading text (not url)
 */
export function strip(str: string, opts?: GenerateOptions): string {
  if (!opts) return str
  if (!opts.strip) return str

  if (typeof opts.strip === "function") {
    return opts.strip(str, opts)
  }

  if (Array.isArray(opts.strip) && opts.strip.length) {
    const res = opts.strip.join("|")
    const re = new RegExp(res, "g")
    str = str.trim().replace(re, "")
    return str.replace(/^-|-$/g, "")
  }

  return str
}
