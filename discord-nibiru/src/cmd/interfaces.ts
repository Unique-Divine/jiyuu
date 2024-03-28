import { Message } from "discord.js"

export interface ICmd {
  msgHandler: SyncMsgHandler | AsyncMsgHandler
  usageCmd: string
  isAsync: boolean
  help: HelpInfo
  payload?: any
}

export type HelpInfo = {
  short: string
  long?: string
}

export const handleStandardCmd = async (
  cmd: ICmd,
  msg: Message,
  mut: { reply: string },
) => {
  let res: CmdResult | undefined
  if (instanceOfICmdAsync(cmd)) {
    res = await cmd.msgHandler(msg)
  } else if (instanceOfICmdSync(cmd)) {
    res = cmd.msgHandler(msg)
  }

  if (res?.reply) {
    mut.reply += "\n" + res?.reply
  }
}

export interface ICmdSync extends ICmd {
  msgHandler: SyncMsgHandler
}

export interface ICmdAsync extends ICmd {
  msgHandler: AsyncMsgHandler
}

type SyncMsgHandler = (msg: Message) => CmdResult | undefined
type AsyncMsgHandler = (msg: Message) => Promise<CmdResult | undefined>

export interface CmdResult {
  /** reply: a reply for the bot to echo back after execution. */
  reply?: string
  /** help: doc string explaining what the command does. */
  help?: string
}

export interface MappedChannelMsg {
  content: string
  createdTimestamp: number
  memberId?: string
}

export interface ChannelKey {
  name: string
  id: string
  type?: string
}

export function instanceOfICmdSync(cmd: ICmd): cmd is ICmdSync {
  return !cmd.isAsync
}

export function instanceOfICmdAsync(cmd: ICmd): cmd is ICmdAsync {
  return cmd.isAsync
}

export enum UsageCmd {
  AllRoles = "all-roles",
  BanAll = "ban-all",
  Echo = "echo",
  Help = "help",
  GrantRoles = "grant-roles",
  Members = "members",
}
