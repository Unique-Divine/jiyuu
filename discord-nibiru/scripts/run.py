from typing import List
import nibi_discord as nd
import discord
import pprint
from discord.ext import commands


USER_NAME = "Vim Diesel#0004"
USER_ID = 181570623993151489

bot = commands.Bot("!", intents=discord.Intents.all())

@bot.event
async def on_ready():
    intents = discord.Intents.all()
    # intents = discord.Intents.default()
    intents.message_content = True

    client = nd.DiscordClient(intents=intents)
    # Start (We won't use connect because we don't want to open a websocket, it will start a blocking loop and it is what we are avoiding)
    await client.login(nd.BOT_TOKEN)

    print("Fixer bot is online.")

@bot.event
async def on_message(msg: discord.Message):
    if msg.author == bot.user:
        return
    
    if msg.author.id != USER_ID:
        return
    
    do_reply: bool = False
    reply = ""
    reply += "Hello, master."
    if "info" in msg.content:
        reply += f"\nmsg.author: {msg.author}"
        reply +=f"\n msg: {msg}"
    if "boi" in msg.content:
        reply += " Boi is here."
        do_reply = True

    if "members" in msg.content:
        guilds = []
        members: List[dict] = []

        for guild in bot.guilds:
            guilds.append(guild)
            for idx, member in enumerate(guild.members):
                members.append(member_to_dict(member))
                if idx == 3:
                    break
            break
        reply += f"guilds: {guilds}"
        reply += f"\nmembers: \n{pprint.pformat(members)}"
    
    if do_reply:
        await msg.reply(reply)
    # if msg.author.id == USER_ID and "!boi" in msg.content:
    #     await msg.reply("hello, boi")

def member_to_dict(member: discord.Member):
    return dict(id=member.id, name=member.name, display_name=member.display_name)    


@bot.command()
async def members(ctx):
    for guild in bot.guilds:
        for member in guild.members:
            await ctx.send(member)

async def task_to_run(): 
    TOKEN = nd.BOT_TOKEN

    intents = discord.Intents.all()
    # intents = discord.Intents.default()
    intents.message_content = True

    client = nd.DiscordClient(intents=intents)
    # Start (We won't use connect because we don't want to open a websocket, it will start a blocking loop and it is what we are avoiding)
    await client.login(TOKEN)
    await client.wait_until_ready()

    for guild in client.guilds:
        for channel in guild.text_channels:
            print(channel.name)
            print(dir(channel))

    # for channel in await client.get_all_channels():
    #     print(channel)

    print("This is working properly.")

    await client.close()
    breakpoint()

def main():

    # out = nd.discord_script(task_to_run)    
    bot.run(nd.BOT_TOKEN)
    ...


if __name__ == "__main__":
    main()