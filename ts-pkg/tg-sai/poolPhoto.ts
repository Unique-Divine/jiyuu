// Sai app icon (NibiruChain/sai-web webapp/public/icon-512.png) flattened
// onto its teal background as a 512x512 JPEG. Embedded by
// `bun build --compile` like the mtcute wasm files.
import photoPath from "./assets/sai-chat-photo.jpg" with { type: "file" }

/** JPEG bytes of the branded Sai photo set on every pool chat. */
export async function poolChatPhoto(): Promise<Uint8Array> {
  return new Uint8Array(await Bun.file(photoPath).arrayBuffer())
}
