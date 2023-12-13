import concat from "concat-stream"
import mdlink from "markdown-link"
import minimist from "minimist"
import merge from "mixin-deep"
import pick from "object.pick"
import repeat from "repeat-string"
import { generate, insert, strip } from "./generate"

export { getTitle, slugify, replaceDiacritics } from "./options"

export { toc } from "./toc"
export const Toc = {
  insert,
  strip,
  plugin: generate,
}
