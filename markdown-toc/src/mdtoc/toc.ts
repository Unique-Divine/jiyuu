import { Token, generate } from "./generate"
import { RemarkablePlus } from "./remarkable"

/**
 * Generates a table of contents (TOC) from markdown headings.
 *
 * @param {string} markdown - Markdown string to generate TOC from
 * @returns {TocResult} TOC result
 */
export const toc = (markdown: string, options?: any): TocResult =>
  new RemarkablePlus({ options })
    .use(generate(options))
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
