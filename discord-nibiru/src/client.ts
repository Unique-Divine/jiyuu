import { Client, Events, GatewayIntentBits, Partials } from "discord.js"

// Create a new client instance
export const CLIENT = new Client({
  intents: [
    GatewayIntentBits.Guilds,
    GatewayIntentBits.GuildMessages, // enables you to interact with messages
    GatewayIntentBits.GuildMessageReactions,
    GatewayIntentBits.MessageContent, // stops message.content from being blank
    GatewayIntentBits.GuildMembers,
  ],
  partials: [Partials.Channel],
})

// When the client is ready, run this code (only once)
// We use 'c' for the event parameter to keep it separate from the already defined 'client'
CLIENT.once(Events.ClientReady, (c) => {
  console.log(`Ready! Logged in as ${c.user.tag}`)
})
