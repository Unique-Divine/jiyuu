"""Optional tool inputs advertise the null defaults handlers already accept."""

from telegram_mcp import runtime, tools  # noqa: F401


def test_null_defaults_are_nullable_and_optional_in_registered_schemas():
    for tool in runtime.mcp._tool_manager.list_tools():
        required = tool.parameters.get("required", [])
        for name, parameter in tool.parameters.get("properties", {}).items():
            if "default" in parameter and parameter["default"] is None:
                assert name not in required, (tool.name, name)
                assert {"type": "null"} in parameter.get("anyOf", []), (tool.name, name)


def test_message_inputs_keep_required_identifiers_and_pagination_defaults():
    tool = next(
        tool for tool in runtime.mcp._tool_manager.list_tools() if tool.name == "get_messages"
    )
    assert tool.parameters["required"] == ["chat_id"]
    properties = tool.parameters["properties"]
    assert properties["chat_id"]["anyOf"] == [{"type": "integer"}, {"type": "string"}]
    assert properties["page"]["default"] == 1
    assert properties["page_size"]["default"] == 20
    assert properties["account"]["default"] is None
