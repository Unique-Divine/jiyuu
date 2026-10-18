import concat from "concat-stream"
import minimist from "minimist"
import { generate, insert, strip } from "./generate"

export { getTitle, slugify, replaceDiacritics } from "./options"
export { toc } from "./toc"
export const Toc = {
  insert,
  strip,
  plugin: generate,
}

export { minimist, concat }
