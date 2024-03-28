// Require the necessary discord.js classes
import {
  Client,
  Events,
  GatewayIntentBits,
  Guild,
  GuildMember,
  Message,
  Partials,
  Role,
  TextChannel,
} from "discord.js"
import { loadDiscordToken } from "./config"
import {
  cmdBan,
  ICmd,
  CmdAllRoles,
  CmdAllChannels,
  CmdBanInChannel,
  CmdGrantRoles,
  CmdResult,
  CmdHelp,
  instanceOfICmdAsync,
  instanceOfICmdSync,
  handleStandardCmd,
  CmdEcho,
  CmdMembers,
} from "./cmd"
import { CLIENT } from "./client"
import { deleteMsgs, getMembers } from "./discord"

/** USER_ID:
 * TODO: Who is this? Likely you. */
const USER_ID = "181570623993151489"

// ----------------------------------------------------------------------
// main - client.on(Events.MessageCreate, msgHandler )
// The heart and soul of the bot lives here.
// Running `ts-node main.ts` initializes a bot that listens for messages from
// USER_ID (Unique).
// ----------------------------------------------------------------------
CLIENT.on(Events.MessageCreate, async (msg: Message) => {
  /** 1. If the bot is running, and a Discord message is submitted by
   * Unique containing the text "boi", then the bot will listen for commands.
   */
  if (msg.author.id !== USER_ID) return
  if (!msg.content.includes("boi")) return

  // reply is populated throughout the course of the function by various commands.
  let reply: string = ""

  const members: GuildMember[] | undefined = await getMembers(msg)

  const cmds: ICmd[] = [
    new CmdAllRoles(),
    new CmdAllChannels(),
    new CmdGrantRoles(msg.guild!),
    new CmdEcho(),
    new CmdMembers(),
  ]
  cmds.push(new CmdHelp(cmds))
  for (const cmd of cmds) {
    const replyObj = { reply }
    await handleStandardCmd(cmd, msg, replyObj)
    reply = replyObj.reply
  }

  // const cmd = new CmdBanInChannel()
  // const resp = await cmd.msgHandler(msg)
  // if (resp) {
  //   const toDelete = resp
  //   for (const [channelId, msgIds] of Object.entries(toDelete)) {
  //     deleteMsgs({ msgIds, channelId })
  //   }
  // }
  // if (cmd.outReply) {
  //   reply += "\n" + cmd.outReply
  // }

  // cmdBan(msg, members)
  // cmdBan(msg, [])

  // console.log("DEBUG Saving member info")
  // saveJson({ obj: members?.map((m: GuildMember) => m.toJSON()) })
  // console.log("DEBUG Saved memberInfos successfully in saveJson.json")

  if (msg.content.includes("echo")) reply += msg.content

  if (reply === "") {
    reply = `I heard your command: ${msg.content}`
  } else {
    reply += `\nmsg.guild: ${msg.guild?.name}`
  }
  await msg.reply(reply)
  return
})

// Log in to Discord with your client's token
CLIENT.login(loadDiscordToken().ok)
