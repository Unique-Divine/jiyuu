import { config as dotenvConfig } from "dotenv"
import { Result } from "./result"
import * as path from "path"

export const loadDiscordToken = (): Result<string> =>
  Result.ofSafeExec(() => {
    dotenvConfig({ path: path.resolve(process.cwd(), ".env") })
    const { TOKEN } = process.env
    if (!TOKEN) {
      const errMsg = `ConfigError: Missing Discord token, "TOKEN", in .env file.`
      console.error(errMsg)
      throw errMsg
    }
    return TOKEN
  })
