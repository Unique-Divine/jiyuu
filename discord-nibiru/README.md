# jiyuu/discord

Implements a Discord bot with powerful moderation commands.

### Main List

- [ ] Cmd: To delete all messages containing a string or list of strings
- [ ] Cmd: To view all counts of messages including a string or list of strings
 - [ ] And save the message info to a local JSON
  

### Project: Adding validator roles. [Complete] 

Objective: Give a `role` to all users that submitted a `message` in `channel` containing a certain `condition`.

- [x] Setup a connection to the Nibiru Chain Discord using discord.js
- [ ] Query message history of `channel`
- [ ] Filter the message history for users that meet the `condition` criterion.
- [ ] Save the array of users | json.dump
- [ ] Use the Discord webhook to send a message the adds a role.
  - [ ] Connect to the webhook to send a "hello world" message using a command.

- [ ] (discord): Grant roles to all testnet-1 complainers
  - [x] Document prior work.
  - [x] Add more type hints if possible.
  - [x] get the list of channels
  - [x] get the messages from the specific channel
  - [x] get the list of roles in the server
  - [ ] grant a role to a single account with a command 
  - [ ] grant a role to a list of accounts

  - [ ] Create an `add_role` command that adds a `role` to some `user` (`addRoleToMember`).
  - [ ] Call `addRoleToMember` on all that should receive the role.
