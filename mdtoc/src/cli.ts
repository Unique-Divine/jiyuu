#!/usr/bin/env bun

import * as fs from "fs"
import { Readable } from "stream"
import { toc, minimist, concat, Toc } from "./mdtoc"
import { TocResult } from "./mdtoc/toc"
// const utils = require("./lib/utils")

/**
 * Parse command-line arguments (`process.argv`) fomr index 2 into `args` using the
 * `minimist` package. Depending on the command-line args, the script outputs
 * the generated table of contents or JSON representation to stdout. If the -i
 * flag is specified, the modified Markdown content is written back to the input
 * file inplace. */
const args: CmdRootArgs = minimist(process.argv.slice(2), {
  // boolean flags: "-i", "--json", ...
  boolean: ["i", "json", "firsth1", "stripHeadingTags"],
  // string flags: Flags that take string args
  string: ["append", "bullets", "indent"],
  // default values for flags
  default: {
    firsth1: true,
    stripHeadingTags: true,
  },
})

type CmdRootArgs = {
  /** i: Toggles whether to write back to the input file in-place. Cannot be used
   * with --json. */
  i: boolean
  /** json: Toggles whether to print the TOC in JSON format */
  json: boolean
  /** firsth1: Toggles whether to include the first `<h1>` element in the TOC.
   * This first heading is often exlcuded and assumed to be the title of the
   * Markdown document. */
  firsth1: boolean
  stripHeadingTags: boolean
  append: string
  /** bullets: The character to use for bullet points. Defaults to "*". */
  bullets: string
  indent: string
  _: string[]
}

// If the number of positional arguments is not exactly 1, the script exits with
// a failure status code and prints to stderr.
if (args._.length !== 1) {
  console.info(
    [
      "Usage: markdown-toc [options] <input> ",
      "",
      "  input:        The Markdown file to parse for table of contents,",
      '                or "-" to read from stdin.',
      "",
      "  -i:           Edit the <input> file directly, injecting the TOC at <!-- toc -->",
      "                (Without this flag, the default is to print the TOC to stdout.)",
      "",
      "  --json:       Print the TOC in JSON format",
      "",
      "  --append:     Append a string to the end of the TOC",
      "",
      "  --bullets:    Bullets to use for items in the generated TOC",
      '                (Supports multiple bullets: --bullets "*" --bullets "-" --bullets "+")',
      '                (Default is "*".)',
      "",
      "  --maxdepth:   Use headings whose depth is at most maxdepth",
      "                (Default is 6.)",
      "",
      "  --no-firsth1: Include the first h1-level heading in a file",
      "",
      "  --no-stripHeadingTags: Do not strip extraneous HTML tags from heading",
      "                         text before slugifying",
      "",
      "  --indent:     Provide the indentation to use - defaults to '  '",
      "                (to specify a tab, use the bash-escaped $'\\t')",
    ].join("\n"),
  )
  process.exit(1)
}

if (args.i && args.json) {
  console.error("markdown-toc: you cannot use both --json and -i")
  process.exit(1)
}

if (args.i && args._[0] === "-") {
  console.error('markdown-toc: you cannot use -i with "-" (stdin) for input')
  process.exit(1)
}

/**
 * `process.stdin` is a `tty.ReadStream` and `fs.createReadStream` returns an
 * `fs.ReadStream`, but both extend `stream.Readable`, so we type the variable
 * with the shared base class.
 * */
let input: Readable = process.stdin
if (args._[0] !== "-") input = fs.createReadStream(args._[0])

input.pipe(
  concat((input: Buffer) => {
    if (args.i) {
      const newMarkdown = Toc.insert(input.toString(), args)
      fs.writeFileSync(args._[0], newMarkdown)
    } else {
      const parsed = toc(input.toString(), args)
      writeToStdOut(parsed, args.json)
    }
  }),
)

input.on("error", (err) => {
  console.error(err)
  process.exit(1)
})

function writeToStdOut(parsed: TocResult, useJson: boolean) {
  if (useJson) return console.log(JSON.stringify(parsed.json, null, "  "))
  process.stdout.write(parsed.content)
}
