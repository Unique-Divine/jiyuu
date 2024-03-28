import { Channel, Collection, Message, TextChannel } from "discord.js"

interface IToDelete {
  [channelId: string]: string[]
}

interface ChannelKey {
  name: string
  id: string
}

class BanChannelHandler {
  adminMsg: Message
  banCount: number
  channelMsgs: Message | null | undefined
  filterString: string
  toDelete: IToDelete
  channelKey: ChannelKey

  constructor(
    adminMsg: Message,
    banCount: number,
    channelKey: ChannelKey,
    toDelete?: IToDelete,
  ) {
    this.adminMsg = adminMsg
    this.banCount = banCount
    this.channelMsgs = undefined
    this.filterString = "hot adult videos?"
    this.channelKey = channelKey

    const initToDelete = { [this.channelKey.name]: [] }
    this.toDelete = toDelete ? { ...toDelete, ...initToDelete } : initToDelete
    this.channelMsgs = this.setupChannelMsgs()
  }

  setupChannelMsgs = async (channel: TextChannel, pageSize = 100) => {
    return await channel.messages.fetch({ limit: pageSize }).then((pageOfMsgs) => {
      return pageOfMsgs.size == pageSize ? pageOfMsgs.at(0) : null
    })
  }

  handleMsg = (channelMsg: Message) => {
    const { content, member } = channelMsg
    if (!content.includes(this.filterString)) return

    // Filter each message for instances of the "filterString"
    console.debug("DEBUG channelMsg.Id:", channelMsg.id)
    console.debug("DEBUG channelMsg:", channelMsg.content)
    console.debug("DEBUG member.displayName:", member?.displayName)
    console.debug("DEBUG member.id:", member?.id)
    this.toDelete[this.channelKey.name].push(channelMsg.id)

    // ban the member if possible
    if (!member) return
    console.log("DEBUG banned user:", member.displayName)
    try {
      this.adminMsg.guild?.members.ban(member.id)
      this.banCount += 1
    } catch (error) {
      console.error(error)
    }
  }

  handlePage = (args: {
    pageOfMsgs: Collection<string, Message>
    pageNumber: number
  }) => {
    const { pageOfMsgs, pageNumber } = args
    pageOfMsgs.forEach((channelMsg) => {
      console.debug(`DEBUG PAGE ${pageNumber}`)
      this.handleMsg(channelMsg)
    })
    this.channelMsgs = 0 < pageOfMsgs.size ? pageOfMsgs.at(pageOfMsgs.size - 1) : null
  }
}
