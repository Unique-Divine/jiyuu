import {
  Channel,
  Guild,
  GuildBasedChannel,
  GuildMember,
  Message,
  Role,
} from "discord.js"
import { addRoleToMember, logModerationAction, roleFromName } from "../discord"
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

    if (!this.role) {
      return {
        reply: "grant-roles: target role not found; aborting",
        help: "Ensure the OG validator role exists before running this command.",
      }
    }

    const execute = msg.content.includes("--execute")
    const dryRun = !execute
    if (dryRun) {
      logModerationAction("grant-roles-dry-run", { requestedBy: msg.author.id })
    }

    const channel: Channel | undefined = msg.guild.client.channels.cache.get(
      this.roleChannel.id,
    )
    if (!channel) {
      return {
        reply: `grant-roles: channel ${this.roleChannel.name} (${this.roleChannel.id}) not found`,
      }
    }

    console.debug("DEBUG PAGE 0:")
    const mappedChannelMsgs: MappedChannelMsg[] = []
    const pageSize = 100

    if (!instanceOfTextChannel(channel)) return

    let channelMsgs = await channel.messages
      .fetch({ limit: pageSize })
      .then((msgPage) => {
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
              if (member) {
                addRoleToMember(member, this.role!, dryRun)
              }
            }
          })
          channelMsgs = 0 < msgPage.size ? msgPage.at(msgPage.size - 1) : null
        })
    }

    const filteredMsgs = mappedChannelMsgs.filter((msgsElem) =>
      msgsElem !== null ? true : false,
    )
    const dumpPath = "grant-roles.json"
    const outputFile = saveJson({
      obj: filteredMsgs,
      fName: dumpPath,
      debugMsg: "DEBUG grant-roles matched messages",
      timestamped: true,
    })

    logModerationAction("grant-roles-summary", {
      requestedBy: msg.author.id,
      matchedMessages: filteredMsgs.length,
      roleId: this.role.id,
      dryRun,
      outputFile,
    })

    return {
      reply: `grant-roles: processed ${filteredMsgs.length} messages; saved details to ${outputFile}. ${
        dryRun
          ? "Dry run; pass --execute to apply roles."
          : "Roles applied to matching members."
      }`,
    }
  }

  getRole = (): Role | undefined => {
    const roleChannel = this.roleChannel
    const roleName: string = "☄️ OG-Validator"
    const role = roleFromName({ name: roleName, guild: this.guild! })
    if (!role) {
      console.warn(
        "grant-roles: unable to find role for channel: " +
          JSON.stringify({ roleName, roleChannel }),
      )
    }
    return role
  }
}

/** TODO doc */
// export function findTestnet1Members(msg: Message): any {
//   if (!msg.guild) return
//   if (!msg.content.includes("all-channels"))
// }
