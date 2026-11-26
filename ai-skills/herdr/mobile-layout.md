# Prepare Tabs for Mobile

Use this workflow when the user wants to continue existing Herdr chats from a
phone. On a small screen, one pane gives the chat room to render. Two evenly
split panes can still work. Three make chat text and scrollback hard to use.
Desktop and phone clients share the workspace layout, so apply this change
when the user asks for it.

1. List tabs and panes in the requested workspace. Default to at most two
   recognized agent chats per tab. Use one chat per tab when requested. Keep
   standalone terminal panes in their original tabs, and report any tab that
   still has more than two total panes because of them.
2. Move only chats above the cap into their own tabs with command
   `herdr pane move <pane-id> --new-tab --no-focus`. Pass `--label` to carry
   the source project's name to each new tab. On the numbered fork, pass the
   source's numbered project label so Herdr recalculates its prefix by position.
   For a one-chat pass, see the existing command in
   [local-machine.md](local-machine.md#flatten-agent-tabs).
3. Group tabs that share a project suffix in the order each project first
   appears. Preserve order within each group. Drag tabs in the desktop tab
   bar or use socket method `tab.move` with the stable `tab_id` and zero-based
   `insert_index`. The index describes a gap in the current order. To move a
   tab rightward into zero-based position `i`, pass `i + 1`.
4. Verify the final tab order, chat counts, and focused tab. Leave the
   running chats attached to their existing pane processes.

For Moshi connection or session discovery, follow the mobile branch of agent
skill `ud`.
