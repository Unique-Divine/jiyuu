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

import { escape as querystringEscape } from "querystring";
import { RemarkablePlus } from "./remarkable";
import { slugify } from "./options";
import markdownLink from "markdown-link";
import { SlugifyOptions, getTitle } from "./options";
import listitem from "list-item";
import { insert } from "./insert";
export { insert };

export { slugify };

interface GenerateOptions extends SlugifyOptions {
  firsth1?: boolean;
  maxdepth?: number;
  linkify?: boolean | LinkifyFn;
  num?: number;
  strip?: boolean | string[] | StripFn;
  titleize?: boolean | TitleizeFn;
  // Add other possible options here...
  hLevel?: number;
  bullets?: string[];
  chars?: string[];
  filter?: FilterFn;
  append?: string;
  highest?: number;
}

type LinkifyFn = (
  tok: Token,
  text: string,
  slug: string,
  options: GenerateOptions,
) => string;

type StripFn = (str: string, opts?: GenerateOptions) => string;

type TitleizeFn = (str: string, options?: GenerateOptions) => string;

/** Returns a boolean indicating whether to filter the item */
type FilterFn = (content: string, ele: Token, arr: Token[]) => boolean;

export interface Token {
  content: string;
  type?: string;
  lvl: number;
  hLevel: number;
  i?: number;
  seen?: number;
  slug?: string;
  children?: Token[];
  lines: number[];
}

export function generate(options?: GenerateOptions): any {
  const opts = { ...{ firsth1: true, maxdepth: 6 }, ...options };
  const stripFirst = opts.firsth1 === false;
  if (opts.linkify === undefined) opts.linkify = true;

  return function (md: RemarkablePlus) {
    md.renderer.render = function (tokens: Token[]) {
      const copiedTokens = [...tokens];
      const seen: { [key: string]: number } = {};
      let tocstart = -1;
      const arr: Token[] = [];
      const res: {
        json?: {
          content: string;
          slug: string;
          lvl: number;
          i: number;
          seen: number;
        }[];
        highest?: number;
        tokens?: Token[];
        content?: string;
      } = {};

      for (let i = 0; i < copiedTokens.length; i++) {
        const token = copiedTokens[i];
        if (/<!--[ \t]*toc[ \t]*-->/.test(token.content)) {
          tocstart = token.lines[1];
        }

        if (token.type === "heading_open") {
          copiedTokens[i + 1].lvl = copiedTokens[i].hLevel;
          copiedTokens[i + 1].i = i;
          arr.push(copiedTokens[i + 1]);
        }
      }

      let result: Token[] = [];
      res.json = [];

      for (const tok of arr) {
        if (tok.lines && tok.lines[0] > tocstart) {
          let val = tok.content;
          if (tok.children && tok.children[0].type === "link_open") {
            if (tok.children[1].type === "text") {
              val = tok.children[1].content;
            }
          }

          if (!seen[val]) {
            seen[val] = 0;
          } else {
            seen[val]++;
          }

          tok.seen = opts.num = seen[val];
          tok.slug = slugify(val, opts);
          res.json.push({
            content: tok.content,
            slug: tok.slug,
            lvl: tok.lvl,
            i: tok.i,
            seen: tok.seen,
          });

          if (opts.linkify) {
            const tokLinkified = linkify(tok, opts);
            result.push(tokLinkified);
          } else {
            result.push(tok);
          }
        }
      }

      opts.highest = highest(result);
      res.highest = opts.highest;
      res.tokens = copiedTokens;

      if (stripFirst) result = result.slice(1);
      res.content = bullets(result, opts);
      if (opts.append) res.content += opts.append;
      return res;
    };
  };
}

/** Func that creates a single markdown list item for the Toc. */
type ListItemFn = (level: number, content: string) => string;

const isListItemFn = (fn: any): fn is ListItemFn => {
  if (typeof fn !== "function") {
    return false; // wrong type of object
  } else if (fn.length !== 2) {
    return false; // wrong number of args
  }

  try {
    const tempResult = fn(1, "test");
    return typeof tempResult === "string";
  } catch (_error) {
    // If calling the func throws an error, it's not a ListItemFn
    return false;
  }
};

const newListitemFn = (
  opts: GenerateOptions,
  fn: FilterFn | null,
): ListItemFn => {
  const liFn = listitem(opts, fn as Function);
  if (isListItemFn(liFn)) return liFn;
  throw Error(`list item fn has the wrong type ${JSON.stringify(liFn)}`);
};

/**
 * Render markdown list bullets
 *
 * @param  {Array} `arr` Array of listitem objects
 * @param  {GenerateOptions} `options`
 * @return {String}
 */
export function bullets(arr: Token[], options?: GenerateOptions): string {
  const opts = {
    indent: "  ",
    ...options,
  };

  opts.chars = opts.chars || opts.bullets || ["-", "*", "+"];

  const unindent = opts.firsth1 === false ? 1 : 0;

  // const li =  =>
  //   listitem(opts);

  const fn = typeof opts.filter === "function" ? opts.filter : null;
  const li: ListItemFn = newListitemFn(opts, fn);

  let res: string[] = [];

  for (let i = 0; i < arr.length; i++) {
    const ele = arr[i];
    ele.lvl -= unindent;
    console.debug("DEBUG obj: %o", {
      ele,
      opts,
    });

    if (fn && !fn(ele.content, ele, arr)) {
      continue;
    }

    const maxDepth = opts.maxdepth ?? 3;
    if (ele.lvl > maxDepth) {
      continue;
    }

    const lvl = ele.lvl - opts.highest;
    const out = li(lvl, ele.content);

    console.debug("DEBUG obj: %o", {
      out,
    });
    res.push(out);
    // res.push(li(lvl, ele.content, opts));
  }

  return res.join("\n");
}

/**
 * Get the highest heading level in the array, so
 * we can un-indent the proper number of levels.
 *
 * @param {Array} `arr` Array of tokens
 * @return {Number} Highest level
 */
export function highest(arr: Token[]): number {
  const sorted = arr.slice().sort((a, b) => a.lvl - b.lvl);

  return sorted.length ? sorted[0].lvl : 0;
}

/**
 * Turn headings into anchors
 */
export function linkify(tok: Token, options?: GenerateOptions): Token {
  const opts = { ...options };
  if (tok && tok.content) {
    opts.num = tok.seen;
    const text = titleize(tok.content, opts);
    const slug = slugify(tok.content, opts);
    const escapedSlug = querystringEscape(slug);

    if (opts && typeof opts.linkify === "function") {
      return opts.linkify(tok, text, escapedSlug, opts);
    }

    tok.content = markdownLink(text, "#" + escapedSlug);
  }
  return tok;
}
/**
 * Titleize the title part of a markdown link.
 */
export function titleize(str: string, opts?: GenerateOptions): string {
  if (opts && opts.strip) {
    return strip(str, opts);
  }

  if (opts && opts.titleize === false) return str;

  if (opts && typeof opts.titleize === "function") {
    return opts.titleize(str, opts);
  }

  str = getTitle(str);

  str = str.split(/<\/?[^>]+>/).join("");

  str = str.split(/[ \t]+/).join(" ");

  return str.trim();
}

/**
 * Optionally strip specified words from heading text (not url)
 */
export function strip(str: string, opts?: GenerateOptions): string {
  if (!opts) return str;
  if (!opts.strip) return str;

  if (typeof opts.strip === "function") {
    return opts.strip(str, opts);
  }

  if (Array.isArray(opts.strip) && opts.strip.length) {
    const res = opts.strip.join("|");
    const re = new RegExp(res, "g");
    str = str.trim().replace(re, "");
    return str.replace(/^-|-$/g, "");
  }

  return str;
}
