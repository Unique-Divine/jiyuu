import * as fs from "fs"
import * as path from "path"

const TMP_DIR = ".tmp"

type SaveJsonArgs = {
  obj: unknown
  debugMsg?: string
  fName?: string
  timestamped?: boolean
}

/** saveJson saves 'obj' as a JSON file.
 *
 * - obj: the object to save
 * - debugMsg: optional description printed to the console.
 * - fName: File name for the JSON file. Defaults to saveJson.json.
 */
export const saveJson = (args: SaveJsonArgs): string => {
  const { obj } = args
  const debugMsg = args.debugMsg ?? "DEBUG Saving JSON payload"
  const baseName = args.fName ?? "saveJson.json"
  const resolvedName = args.timestamped ? addTimestampSuffix(baseName) : baseName
  const outputPath = path.join(TMP_DIR, resolvedName)

  ensureDir(path.dirname(outputPath))

  console.debug(`${debugMsg} in ${outputPath}`)
  fs.writeFileSync(outputPath, JSON.stringify(obj, undefined, 2) + "\n", {
    encoding: "utf8",
  })
  return outputPath
}

const ensureDir = (dirPath: string) => {
  if (!fs.existsSync(dirPath)) {
    fs.mkdirSync(dirPath, { recursive: true })
  }
}

/** Append an ISO timestamp (safe for filenames) before the file extension. */
const addTimestampSuffix = (fileName: string): string => {
  const ts = new Date().toISOString().replace(/[:.]/g, "-")
  const ext = path.extname(fileName)
  const name = ext.length > 0 ? fileName.slice(0, -ext.length) : fileName
  return `${name}-${ts}${ext}`
}
