import {
  Channel,
  Collection,
  FetchMessagesOptions,
  GuildMember,
  Message,
  TextChannel,
} from "discord.js"
import { CHANNEL_KEY_MAP } from "../channels"
import { deleteMsgs, iterMsgs, PAGE_SIZE } from "../discord"
import { instanceOfTextChannel } from "../typing"
import { ChannelKey, ICmd, MappedChannelMsg, UsageCmd } from "./interfaces"

export function cmdBan(
  msg: Message,
  members: GuildMember[] | undefined,
): { reply: string } {
  let reply: string
  let banCount = 0
  const bans: string[] = []

  if (msg.content.includes("ban")) {
    members?.forEach((mem: GuildMember) => {
      const banString = "hot adult videos?"
      // const banString = "®"
      const shouldBan = (gm: GuildMember): boolean => gm.displayName.includes(banString)
      if (shouldBan(mem)) {
        console.log("DEBUG banned user:", mem.displayName)
        msg.guild?.members.ban(mem.id)
        banCount += 1
        bans.push(mem.displayName)
      }
    })

    if (banCount !== 0) {
      reply = `I just banned ${banCount} Discord members`
      reply += `\nbanned members: ${bans}`
    } else {
      reply = "No members met the ban criterion."
    }
  } else {
    reply = ""
  }
  return { reply }
}

interface IToDelete {
  [channelId: string]: string[]
}

type ContentFilterFn = (content: string) => boolean

const filterStrings = ["discord.gg/teddyru", "discord.gg/fraith"]

const FILTER_FN: ContentFilterFn = (content: string): boolean => {
  const bads: boolean[] = filterStrings.map((badWord) => content.includes(badWord))
  return bads.some(Boolean)
}

/** deleteMsgsFiltered: Iterate through a series of messages and delete them
 * if they contain the filter string.
 * */
const deleteMsgsFiltered = ({
  msgs,
  channel,
  filterFn,
  msgIdsToDelete,
  isDebugMode,
}: {
  msgs: Collection<string, Message>
  channel: TextChannel
  filterFn: ContentFilterFn
  msgIdsToDelete: string[]
  isDebugMode?: boolean
}) => {
  if (isDebugMode) {
    console.debug("DEBUG processMsgs: msg count:", msgs.size)
  }
  let idx = 0
  for (const msg of msgs) {
    if (filterFn(msg[1].content)) {
      const msgId = msg[1].id

      if (isDebugMode) {
        console.debug(`DEBUG msg ${idx}:`, msg[1].content)
        console.debug(`DEBUG msgId:`, msgId)
      }
      msgIdsToDelete.push(msgId)
    }
    idx += 1
  }
  deleteMsgs({ msgIds: msgIdsToDelete, channelId: channel.id })
}

export class CmdBanInChannel implements ICmd {
  usageCmd: string
  outReply: string
  banCount: number
  deleteCount: number
  isAsync = true
  help = { short: `Bans all guild members based on a "bad message" filter` }

  constructor() {
    this.usageCmd = UsageCmd.BanAll.valueOf()
    this.outReply = ""
    this.banCount = 0
    this.deleteCount = 0
  }

  /** handleChannel: Returns an object containing the IDs of messages to delete
   * and their corresponding channel IDs.
   *
   * Iterate through pages of messages doing the following:
   * 1. Check if messages containa filter criterion, usually indicative of spam
   * or unwanted content.
   * 2. Queue up these messages for deletion by saving their IDs. These are
   * stored in the collection called "allMessages".
   * */
  handleChannel = async (args: {
    channelKey: ChannelKey
    msg: Message
  }): Promise<{ msgIdsToDelete: string[]; channelId: string } | void> => {
    const { channelKey, msg } = args
    this.outReply += `Commencing purge of spam in the "${channelKey.name}" channel...`

    if (!msg.guild) return
    if (!msg.content.includes(this.usageCmd)) return

    const channel: Channel | undefined = msg.guild.client.channels.cache.get(
      channelKey.id,
    )
    if (!channel) return

    console.debug("DEBUG PAGE 0:")
    const mappedChannelMsgs: MappedChannelMsg[] = []

    if (!instanceOfTextChannel(channel)) return

    const msgIdsToDelete: string[] = []

    let allMessages = new Collection<string, Message>() // Collection to store all messages

    async function fetchMessages(before: string | undefined | null) {
      console.debug("DEBUG fetchMessages...")
      if (!instanceOfTextChannel(channel!)) return
      const options: FetchMessagesOptions = {
        limit: PAGE_SIZE,
        before: before ?? undefined,
      }
      channel.messages
        .fetch(options)
        .then(async (messages) => {
          if (messages.size === 0) {
            // All messages have been fetched
            console.log(`Fetched ${allMessages.size} messages in total`)
            deleteMsgsFiltered({
              msgs: allMessages,
              channel,
              filterFn: FILTER_FN,
              msgIdsToDelete,
            })
            return
          }

          // Add the new messages to the collection
          allMessages = allMessages.concat(messages)

          // Recursively fetch the next page of messages
          await fetchMessages(messages.last()?.id)
        })
        .catch((err) => {
          console.error(err)
        })
    }

    // await fetchMessages(null) // Start fetching messages from the beginning
    await iterMsgs({
      before: null,
      channel,
      msgHandler: async (msgs) => {
        if (msgs.size !== 0) {
          // Add the new messages to the collection
          allMessages = allMessages.concat(msgs)
          return
        }

        // All messages have been fetched
        console.log(`Fetched ${allMessages.size} messages in total`)
        deleteMsgsFiltered({
          msgs: allMessages,
          channel,
          filterFn: FILTER_FN,
          msgIdsToDelete,
        })
        return
      },
    })

    return { msgIdsToDelete, channelId: channel.id }

    // while (channelMsgs) {
    //   console.debug("DEBUG currPage:", currPage)
    //   console.debug("DEBUG lastPage:", lastPage)
    //   // currPage += 1

    //   await channel.messages
    //     .fetch({ limit: pageSize, before: channelMsgs.id })
    //     .then((msgPage) => {
    //       msgPage.forEach((channelMsg) => {
    //       console.debug("DEBUG channelMsg.content:", channelMsg.content)
    //       if (currPage !== lastPage) {
    //         console.debug(`DEBUG PAGE ${currPage}`)
    //         lastPage = currPage
    //       }
    //       const { content, createdTimestamp, member } = channelMsg

    //       // Filter each message for instances of the "filterString"
    //       if (content.includes(filterString)) {
    //         console.debug("DEBUG channelMsg.Id:", channelMsg.id)
    //         console.debug("DEBUG channelMsg:", channelMsg.content)
    //         console.debug("DEBUG member.displayName:", member?.displayName)
    //         console.debug("DEBUG member.id:", member?.id)
    //         mappedChannelMsgs.push({
    //           content,
    //           createdTimestamp,
    //           memberId: member?.id,
    //         })
    //         msgIdsToDelete.push(channelMsg.id)

    //         // ban the member if possible
    //         if (member) {
    //           console.log("DEBUG banned user:", member.displayName)
    //           try {
    //             msg.guild?.members.ban(member.id)
    //             this.banCount += 1
    //           } catch (error) {
    //             console.error(error)
    //           }
    //         }
    //       }
    //     })
    //     currPage += 1
    //     channelMsgs = 0 < msgPage.size ? msgPage.at(msgPage.size - 1) : null
    //   })
    // this.deleteCount += msgIdsToDelete.length
    // if (this.banCount !== 0) {
    //   this.outReply += `I just banned ${this.banCount} Discord members 🔥.`
    // } else {
    //   this.outReply += "No members met the ban criterion."
    // }
    // if (this.deleteCount !== 0) {
    //   this.outReply += `\nDeleting ${this.deleteCount} spam messages 🔥.`
    // }
    // console.debug("DEBUG msgIdsToDelete: %o", msgIdsToDelete)

    // saveJson({
    //   obj: mappedChannelMsgs.filter((msgsElem) => (msgsElem !== null ? true : false)),
    //   fName: "bans.json",
    // })
    // return { msgIdsToDelete, channelId: channel.id }
    // }
  }

  /** Returns the msg Ids that need to be deleted. */
  msgHandler = async (msg: Message): Promise<IToDelete | undefined> => {
    if (!msg.guild) return
    if (!msg.content.includes(this.usageCmd)) return

    console.info("DEBUG Beginning CmdBanInChannel.msgHandler...")
    const channelKeys: ChannelKey[] = [CHANNEL_KEY_MAP.spam]

    const toDelete: IToDelete = {}
    for (const channelKey of channelKeys) {
      const handleResp = await this.handleChannel({ channelKey, msg })
      console.debug("DEBUG handleResp: %o", handleResp)
      if (!handleResp) return
      toDelete[handleResp.channelId] = handleResp.msgIdsToDelete
    }
    console.debug("DEBUG toDelete %o:", toDelete)
    return toDelete
  }
}
