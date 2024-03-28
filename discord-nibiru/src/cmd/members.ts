import { Message } from "discord.js"
import { iterGuildMembers } from "../discord"
import { CmdResult, ICmd, UsageCmd } from "./interfaces"

/** CmdMembers: TODO */
export class CmdMembers implements ICmd {
  usageCmd: string
  isAsync = true
  help = { short: "TODO" }

  constructor() {
    this.usageCmd = UsageCmd.Members.valueOf()
  }

  msgHandler = async (msg: Message): Promise<CmdResult | undefined> => {
    if (!msg.guild) return
    if (!msg.content.includes(this.usageCmd)) return
    const reply = "Called CmdMembers: \n"
    await iterGuildMembers({ guild: msg.guild, memberHandler: () => {} })
    return { reply, help: this.help.short }
  }
}
