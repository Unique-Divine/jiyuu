import {
  Channel,
  Collection,
  FetchMembersOptions,
  FetchMessagesOptions,
  Guild,
  GuildMember,
  Message,
  Role,
  TextChannel,
} from "discord.js"
import { CLIENT } from "./client"
import { instanceOfTextChannel } from "./typing"

export function addRoleToMember(member: GuildMember, role: Role) {
  member.roles.add(role)
}

export async function getMembers(msg: Message): Promise<GuildMember[] | undefined> {
  const members = await msg.guild?.members.fetch()
  if (members) {
    return members.map((mem: GuildMember) => mem)
  }
  return undefined
}

/** Searches a `Guild` for a role using its name. Returns undefined if the role
 * is not found.
 * */
export function roleFromName(args: { name: string; guild: Guild }): Role | undefined {
  const { name, guild } = args
  return guild.roles.cache.find((guildRole) => guildRole.name === name)
}

/** deleteMsgs: Delete a collection of messages by message ID in the given
 * channel.
 * */
export function deleteMsgs(args: { msgIds: string[]; channelId: string }) {
  const { channelId, msgIds } = args
  const channel = CLIENT.channels.cache.get(channelId) as TextChannel
  msgIds.forEach((msgId) => {
    console.debug("DEBUG msgId:", msgId)
    channel.messages
      .fetch(msgId)
      .then((msg) => {
        msg.delete()
      })
      .catch((err) => console.error(err))
  })
}

export const PAGE_SIZE = 100 // Number of messages to fetch per page
/** The default limit is 50. The maximum allowed one is 100. */

type FnChannelMsgHandler = (
  msgs: Collection<string, Message<true>>,
  ...args: unknown[]
) => Promise<void>

/** TODO: doc
 *
 * @example fetch messages from the beginning of the channel
 * ```ts
 * await iterMsgs({ before: null })
 * ```
 *
 * */
export async function iterMsgs({
  before,
  channel,
  msgHandler,
}: {
  before: string | undefined | null
  channel: Channel
  msgHandler: FnChannelMsgHandler
}) {
  console.debug("DEBUG fetchMessages...")
  if (!instanceOfTextChannel(channel!)) return
  const options: FetchMessagesOptions = {
    limit: PAGE_SIZE,
    before: before ?? undefined,
  }

  let msgCounter = 0
  channel.messages
    .fetch(options)
    .then(async (messages) => {
      msgHandler(messages)
      if (messages.size === 0) {
        console.log(`fetchMsgs: Fetched ${msgCounter} messages in total`)
        return
      }

      msgCounter += messages.size
      // Add the new messages to the collection
      // allMessages = allMessages.concat(messages)

      // Recursively fetch the next page of messages
      await iterMsgs({ before: messages.last()?.id, channel, msgHandler })
    })
    .catch((err) => {
      console.error(err)
    })
}

type GuildMembersHandler = (members: GuildMember[]) => void

export async function iterGuildMembers({
  guild,
  memberHandler,
  pageLimit = PAGE_SIZE,
}: {
  guild: Guild
  memberHandler: GuildMembersHandler
  pageLimit?: number
}) {
  // TODO: try, catch
  let lastTs: number | undefined = undefined
  let moreMembsAvailable = true

  const fullMembsList: GuildMember[] = []

  let iterIdx = 0
  // TODO: track idx for pages so that I can increment
  try {
    while (moreMembsAvailable) {
      const opts: FetchMembersOptions = { limit: pageLimit }
      // const opts: FetchMembersOptions = { limit: pageLimit, time: lastTs }
      const membs: Collection<string, GuildMember> = await guild.members.fetch(opts)
      if (membs.size > 0) {
        const membsList: GuildMember[] = Array.from(membs.values())
        fullMembsList.concat(membsList)
        memberHandler(membsList)
        moreMembsAvailable = membs.size === pageLimit
        lastTs = membs.last()?.joinedTimestamp ?? undefined
      } else {
        moreMembsAvailable = false
      }
      iterIdx += 1
      if (iterIdx >= 3) {
        console.debug("DEBUG manual stop %o:", { fullMembsList })
      }
    }
  } catch (err) {
    console.error("iterGuildMembers: ", err)
  }

  // TODO: grab batch of members
  // TODO: run handler
  // TODO: increment track idx
}
