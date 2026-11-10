// The repository's locked Bun type package predates Bun 1.4's YAML parser.
import "bun"

declare module "bun" {
  export const YAML: { parse(input: string): unknown }
}
