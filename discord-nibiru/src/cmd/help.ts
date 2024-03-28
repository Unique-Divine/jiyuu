import { Message } from "discord.js"
import { CmdResult, ICmd, UsageCmd } from "./interfaces"

/** CmdAllRoles: displays all of the guild roles. */
export class CmdHelp implements ICmd {
  usageCmd: string
  cmds: ICmd[]
  outReply: string = ""
  help = { short: "Describe each of the commands." }

  constructor(cmds: ICmd[]) {
    this.usageCmd = UsageCmd.Help.valueOf()
    this.cmds = cmds
  }

  msgHandler = (msg: Message): CmdResult | undefined => {
    if (!msg.guild) return
    if (!msg.content.includes(this.usageCmd)) return

    const helpLines = this.cmds.map((cmd) => {
      return `${cmd.usageCmd}: ${cmd.help.short}`
    })
    const reply = "Bot Commands: \n" + helpLines.join("\n")
    this.outReply += reply
    return { reply, help: this.help.short }
  }

  isAsync = false
}
