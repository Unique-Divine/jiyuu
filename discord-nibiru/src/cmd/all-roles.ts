import { ChannelType, GuildBasedChannel, Message, Role } from "discord.js"
import { saveJson } from "./utils"
import { CmdResult, ICmd, UsageCmd } from "./interfaces"

/** CmdAllRoles: displays all of the guild roles. */
export class CmdAllRoles implements ICmd {
  usageCmd: string
  roleNames: string
  outReply: string
  help = { short: "displays all of the guild roles." }

  constructor() {
    this.usageCmd = UsageCmd.AllRoles.valueOf()
    this.outReply = ""
    this.roleNames = ""
  }

  msgHandler = (msg: Message): CmdResult | undefined => {
    if (!msg.guild) return
    if (!msg.content.includes(this.usageCmd)) return
    const roleNames = msg.guild.roles.cache.map((role: Role) => role.name)
    this.roleNames = JSON.stringify(roleNames, undefined, 2)
    return { reply: "roles: " + this.roleNames }
    this.outReply += "roles: " + this.roleNames
  }

  isAsync = false
}

/** CmdAllChannels: displays all of the channel info for the guild. */
export class CmdAllChannels implements ICmd {
  usageCmd: string
  channels: { name: string; id: string; type: string }[]
  outReply: string
  help = { short: "displays all of the channel info for the guild." }

  constructor() {
    this.usageCmd = "all-channels"
    this.outReply = ""
    this.channels = []
  }

  msgHandler = (msg: Message): CmdResult | undefined => {
    if (!msg.guild) return
    if (!msg.content.includes("all-channels")) return
    this.channels = msg.guild.channels.cache.map((channel: GuildBasedChannel) => {
      const { name, id, type } = channel
      let channelType: string = ""
      for (const [typeName, enumValue] of Object.entries(ChannelType)) {
        if (enumValue === type) {
          channelType = typeName
        }
      }
      return { name, id, type: channelType }
    })
    const fName = "channels.json"
    saveJson({ obj: this.channels, fName: "channels.json" })
    return { reply: `channels: saved channels to ${fName}` }
  }

  isAsync = false
}
