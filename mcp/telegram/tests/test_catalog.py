"""The public tool catalog lists every registered tool with its hints."""

import asyncio

from telegram_mcp import catalog, runtime


def test_catalog_matches_registered_tools():
    listed = asyncio.run(catalog.list_tools())
    registered = runtime.mcp._tool_manager.list_tools()
    assert [tool.name for tool in listed] == [tool.name for tool in registered]
    assert "mcp_info" in {tool.name for tool in listed}


def test_catalog_tools_carry_titles_and_schemas():
    for tool in asyncio.run(catalog.list_tools()):
        assert tool.annotations is not None, tool.name
        assert tool.annotations.title, tool.name
        assert tool.inputSchema.get("type") == "object", tool.name
