import { SlugifyOptions } from "./options";
import { toc } from "./toc";
import matter from "gray-matter";

function split(str: string, re: RegExp) {
  return str.split(re).map(trim);
}

function trim(str: string) {
  return str.trim();
}

interface InsertOptions extends SlugifyOptions {
  /**  Regex to find toc insertion points  */
  regex?: RegExp;

  /** Opening tag for toc */
  open?: string;

  /** Closing tag for toc */
  close?: string;

  /** Custom toc content to insert */
  toc?: string;

  linkify?: boolean;
}

const defaultInsertOptions: InsertOptions = {
  regex: /(?:<!-- toc(?:\s*stop)? -->)/g,
  open: "<!-- toc -->\n\n",
  close: "<!-- tocstop -->",
};

/**
 * The basic idea:
 *
 *  1. when front-matter exists, we need to avoid turning its properties into headings.
 *  2. We need to detect toc markers on the page. For now it's a simple HTML code comment
 *     to ensure the markdown is compatible with any parser.
 *
 * @param  {String} `str` Pass a string of markdown
 * @param  {Object} `options` Pass options to toc generation
 * @return {String} Get the same string back with a TOC inserted
 */
export function insert(str: string, options?: InsertOptions): string {
  options = { ...defaultInsertOptions, ...options };

  let { regex, open, close } = options;
  if (!close) close = defaultInsertOptions.close!;

  var newlines = "";
  var m = /\n+$/.exec(str);
  if (m) newlines = m[0];

  let obj: MatterFile | null = null;
  // does the file have front-matter?
  if (/^---/.test(str)) {
    // extract it temporarily so the syntax
    // doesn't get mistaken for a heading
    obj = grayMatter(str);
    str = obj.content;
  }

  var sections = split(str, regex ?? defaultInsertOptions.regex!);
  if (sections.length > 3) {
    throw new Error(
      "markdown-toc only supports one Table of Contents per file.",
    );
  }

  var last = sections[sections.length - 1];
  if (sections.length === 3) {
    sections.splice(1, 1, open + (options.toc || toc(last, options).content));
    sections.splice(2, 0, close);
  }

  if (sections.length === 2) {
    sections.splice(1, 0, open + toc(last, options).content + "\n\n" + close);
  }

  var resultString = sections.join("\n\n") + newlines;
  // if front-matter was found, put it back now
  if (obj) {
    return matter.stringify(resultString, obj.data);
  }
  return resultString;
}

const grayMatter = (
  str: string,
  options?: matter.GrayMatterOption,
): MatterFile => {
  return matter(str, options) as MatterFile;
};

/**
 * Object returned by the `matter` function from "gray-matter"
 */
export interface MatterFile {
  /** Parsed front matter data */
  data: { [key: string]: any };

  /** Content string without front matter */
  content: string;

  /** Excerpt extracted from content */
  excerpt: string;

  /** Original unmodified input */
  orig: string;

  /** Language defined in front matter */
  language?: string;

  /** Raw front matter string */
  matter?: string;

  /** File path if read from the file system */
  path?: string;
}

// function split(str, re) {
//   return str.split(re).map(trim);
// }
