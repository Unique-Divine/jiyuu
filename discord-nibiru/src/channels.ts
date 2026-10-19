import { type ChannelKey } from "./cmd/interfaces"

export const CHANNEL_KEY_MAP: { [key: string]: ChannelKey } = {
  welcome: {
    name: "👋︲welcome",
    id: "947912808664801322",
    type: "GuildText",
  },
  chinese: {
    name: "🌏︲nibiru-中文",
    id: "1036141044607242291",
    type: "GuildText",
  },
  turkish: {
    name: "🌏︲nibiru-turkish",
    id: "1037785182838738954",
    type: "GuildText",
  },
  russian: {
    name: "🌏︲nibiru-русский",
    id: "1037090097264336976",
    type: "GuildText",
  },
  spam: {
    name: "🔪︲report-spam",
    id: "1056251480321036289",
    type: "GuildText",
  },
  memes: {
    name: "🐸︲memes-and-off-topic",
    id: "947912839841054730",
  },
  listeningReccs: {
    name: "🎧︲listening-reccs",
    id: "1020067964759855265",
    type: "GuildText",
  },
  gm: {
    name: "🌅︲gm-gn",
    id: "947913912014233621",
    type: "GuildText",
  },
  general: {
    name: "💬︲general",
    id: "947911972341579899",
    type: "GuildText",
  },
  amaQuestions: {
    name: "ama-questions",
    id: "1083475931747864628",
    type: "GuildText",
  },
  indonesia: {
    name: "🌏︲nibiru-indonesia",
    id: "1055291771409682433",
    type: "GuildText",
  },
}

export const CHANNEL_KEYS_ALL = Object.values(CHANNEL_KEY_MAP)
