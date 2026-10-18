declare module "markdown-link" {
  export default function link(anchor: string, href: string, title?: string): string
}

declare module "list-item" {
  export default function listItem(
    options: { indent?: string; chars?: string | string[]; nobullet?: boolean },
    transform?: Function | null,
  ): (level: number, content: string) => string
}

declare module "diacritics-map" {
  const diacritics: Record<string, string>
  export default diacritics
}
