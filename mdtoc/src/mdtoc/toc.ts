import { GenerateOptions, Token, generate } from "./generate"
import { RemarkablePlus } from "./remarkable"

/**
 * Generates a table of contents (TOC) from markdown headings.
 *
 * @param {string} markdown - Markdown string to generate TOC from
 * @param {GenerateOptions} opts - Options for TOC generation
 * @returns {TocResult} TOC result
 */
export const toc = (markdown: string, opts?: GenerateOptions): TocResult =>
  new RemarkablePlus({})
    .use(generate(opts))
    .render(markdown) as unknown as TocResult

/** Table of contents (TOC) result */
export interface TocResult {
  /** Rendered TOC content string */
  content: string

  /** Highest heading level found */
  highest: number

  /** Array of markdown AST tokens */
  tokens: Token[]

  /** Array of heading objects */
  json: Heading[]
}

/** Heading object */
interface Heading {
  /** Heading content text */
  content: string

  /** Heading level */
  lvl: number

  /** Slugified head ID */
  slug: string
}
