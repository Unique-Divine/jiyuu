import { Channel, ChannelType, TextChannel } from "discord.js"

export function instanceOfTextChannel(channel: Channel): channel is TextChannel {
  return channel.type === ChannelType.GuildText
}
