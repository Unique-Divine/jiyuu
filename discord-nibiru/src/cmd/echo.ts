import { Message } from "discord.js"
import { CmdResult, ICmd, UsageCmd } from "./interfaces"

/** CmdEcho: Bot command that replies back with whatever was sent. */
export class CmdEcho implements ICmd {
  usageCmd: string = UsageCmd.Echo.valueOf()
  isAsync = false
  help = { short: "Bot command that replies back with whatever was sent" }

  constructor() {}

  msgHandler = (msg: Message): CmdResult | undefined => {
    if (!msg.guild) return
    if (!msg.content.includes(this.usageCmd)) return
    return { reply: msg.content }
  }
}
