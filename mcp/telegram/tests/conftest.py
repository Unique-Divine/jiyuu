"""Shared pytest setup for import-time Telegram configuration."""

import os

import pytest

os.environ.setdefault("TELEGRAM_API_ID", "12345")
os.environ.setdefault("TELEGRAM_API_HASH", "dummy_hash")
os.environ.setdefault("TELEGRAM_SESSION_NAME", "test_session")


@pytest.fixture(autouse=True)
def _clear_request_cooldowns():
    """Keep cooldowns recorded by one test out of the next."""
    from telegram_mcp.request_policy import request_cooldowns

    request_cooldowns.clear()
    yield
    request_cooldowns.clear()
