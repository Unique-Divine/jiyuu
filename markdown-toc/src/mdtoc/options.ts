/**
 * src/options.ts
 */
import stripColor from "strip-color"
import diacritics from "diacritics-map"

export { getTitle, replaceDiacritics, slugify, defaultOptions }
export interface SlugifyOptions {
  slugify?: SlugifyFn | boolean
  stripHeadingTags?: boolean
  num?: number
}

export type SlugifyFn = (str: string, options: SlugifyOptions) => string

/**
 * Get the "title" from a markdown link
 */
function getTitle(str: string): string {
  if (/^\[[^\]]+\]\(/.test(str)) {
    const m = /^\[([^\]]+)\]/.exec(str)
    if (m) return m[1]
  }
  return str
}

const defaultOptions: SlugifyOptions = {
  slugify: (str: string, _options: SlugifyOptions) => str,
  stripHeadingTags: false,
  num: undefined,
}

/**
 * Slugify the url part of a markdown link.
 *
 * @name  options.slugify
 * @param  {String} `str` The string to slugify
 * @param  {Object} `options` Pass a custom slugify function on `options.slugify`
 * @return {String}
 * @api public
 */
function slugify(str: string, options?: SlugifyOptions): string {
  if (!options) options = defaultOptions

  if (options.slugify === false) return str
  if (typeof options.slugify === "function") {
    return options.slugify(str, options)
  }

  str = getTitle(str)
  str = stripColor(str)
  str = str.toLowerCase()

  // `.split()` is often (but not always) faster than `.replace()`
  str = str.split(" ").join("-")
  str = str.split(/\t/).join("--")
  if (options.stripHeadingTags !== false) {
    str = str.split(/<\/?[^>]+>/).join("")
  }
  str = str.split(/[|$&`~=\\\/@+*!?({[\]})<>=.,;:'"^]/).join("")
  str = str
    .split(/[。？！，、；：“”【】（）〔〕［］﹃﹄“ ”‘’﹁﹂—…－～《》〈〉「」]/)
    .join("")
  str = replaceDiacritics(str)
  if (options.num) {
    str += `-${options.num}`
  }
  return str
}

function replaceDiacritics(str: string) {
  return str.replace(/[À-ž]/g, (ch) => diacritics[ch] || ch)
}
