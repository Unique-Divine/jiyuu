import * as fs from "fs"

/** saveJson saves 'obj' as a JSON file.
 *
 * - obj: the object to save
 * - debugMsg: optional description printed to the console.
 * - fName: File name for the JSON file. Defaults to saveJson.json.
 */
export function saveJson(args: { obj: any; debugMsg?: string; fName?: string }) {
  if (!args.fName) args.fName = "saveJson.json"
  if (!args.debugMsg) args.debugMsg = "DEBUG Saving member info"
  console.debug(`${args.debugMsg} in ${args.fName}`)
  fs.appendFileSync(args.fName, JSON.stringify(args.obj, undefined, 2))
}

