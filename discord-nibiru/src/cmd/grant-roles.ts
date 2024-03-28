import {
  Channel,
  Guild,
  GuildBasedChannel,
  GuildMember,
  Message,
  Role,
} from "discord.js"
import { addRoleToMember, roleFromName } from "../discord"
import { instanceOfTextChannel } from "../typing"
import { CmdResult, ICmd, MappedChannelMsg, UsageCmd } from "./interfaces"
import { saveJson } from "./utils"

/** CmdGrantRoles: Searches the "validator-role-request" channel and adds
 * the role, "☄️ Validator" , to all eligible Guild  members.
 * */
export class CmdGrantRoles implements ICmd {
  usageCmd: string
  outReply: string = "Successfully granted roles to everyone."
  help = { short: "Grant OG validator roles to eligible guild members" }

  // Custom fields
  role?: Role
  guild: Guild
  roleChannel = { name: "💬︲general", id: "947911972341579899" }

  constructor(guild: Guild) {
    this.usageCmd = UsageCmd.GrantRoles.valueOf()
    this.guild = guild
    this.role = this.getRole()
  }

  isAsync = true

  onMsgHandleReply = (): string => "Successfully granted roles to everyone."

  msgHandler = async (msg: Message): Promise<CmdResult | undefined> => {
    if (!msg.guild) return
    if (!msg.content.includes(this.usageCmd)) return

    console.debug("DEBUG Assigning roles for role: ", this.role?.name)

    const channel: Channel | undefined = msg.guild.client.channels.cache.get(
      this.roleChannel.id,
    )
    if (!channel) return

    console.debug("DEBUG PAGE 0:")
    const mappedChannelMsgs: MappedChannelMsg[] = []
    const pageSize = 100

    if (!instanceOfTextChannel(channel)) return

    let channelMsgs = await channel.messages.fetch({ limit: 100 }).then((msgPage) => {
      console.debug("DEBUG msgPage.size:", msgPage.size)
      return msgPage.size == pageSize ? msgPage.at(0) : null
    })

    console.debug("DEBUG PAGE 1:")
    let page = 1

    while (channelMsgs) {
      page += 1
      await channel.messages
        .fetch({ limit: pageSize, before: channelMsgs.id })
        .then((msgPage) => {
          msgPage.forEach((channelMsg) => {
            console.debug(`DEBUG PAGE ${page}`)
            const { content, createdTimestamp, member } = channelMsg

            // Filter each message for instances of "nibi"
            if (content.includes("nibi")) {
              console.debug("DEBUG channelMsg:", channelMsg.content)
              console.debug("DEBUG member.displayName:", member?.displayName)
              mappedChannelMsgs.push({ content, createdTimestamp, memberId: member?.id })
              if (member && this.role) {
                addRoleToMember(member, this.role)
              }
            }
          })
          channelMsgs = 0 < msgPage.size ? msgPage.at(msgPage.size - 1) : null
        })
    }

    saveJson({
      obj: mappedChannelMsgs.filter((msgsElem) => (msgsElem !== null ? true : false)),
    })

    return
  }

  getRole = (): Role | undefined => {
    const roleChannel = this.roleChannel
    const roleName: string = "☄️ OG-Validator"
    const role = roleFromName({ name: roleName, guild: this.guild! })
    console.error(
      "unable to find role for channel: " + JSON.stringify({ roleName, roleChannel }),
    )
    return role
  }
}

/** TODO doc */
// export function findTestnet1Members(msg: Message): any {
//   if (!msg.guild) return
//   if (!msg.content.includes("all-channels"))
// }
