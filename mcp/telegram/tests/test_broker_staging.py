import os

import pytest

from telegram_mcp.bot_identity import BotIdentity, for_broker
from telegram_mcp.session_store import State, StoreError


def test_staging_requires_disposable_pins_and_isolated_state(monkeypatch, tmp_path):
    root = tmp_path / "stage"
    monkeypatch.setenv("BOKU_HOME", os.fspath(root))
    state = State.discover()
    env = {
        "BOKU_HOME": os.fspath(root),
        "SAI_STAGING_BOT_ID": "41",
        "SAI_STAGING_CUSTODY_ID": "42",
        "SAI_STAGING_OWNER_ID": "43",
        "SAI_STAGING_BOT_USERNAME": "FixtureBot",
    }
    with pytest.raises(StoreError, match="production Telegram identities"):
        for_broker(state, {43}, env | {"SAI_STAGING_BOT_ID": "8983431101"})
    with pytest.raises(StoreError, match="production Telegram identities"):
        for_broker(state, {43}, env | {"SAI_STAGING_OWNER_ID": "1912700300"})
    with pytest.raises(StoreError, match="pinned disposable owner"):
        for_broker(state, {44}, env)
    with pytest.raises(StoreError, match="outside its pinned bot identity"):
        for_broker(state, {43}, env | {"SDD_TELEGRAM_BOT_TOKEN": "8983431101:fake"})
    with pytest.raises(StoreError, match="separate explicit BOKU_HOME"):
        for_broker(state, {43}, env | {"BOKU_HOME": ""})
    assert for_broker(state, {43}, env) == BotIdentity(41, "FixtureBot")
    with pytest.raises(StoreError, match="different environment"):
        for_broker(state, {43}, {})


def test_default_broker_pins_production_bot_only(monkeypatch, tmp_path):
    root = tmp_path / "boku"
    monkeypatch.setenv("BOKU_HOME", os.fspath(root))
    state = State.discover()
    assert for_broker(state, {43}, {}) == BotIdentity(8983431101, "SaiTeam_bot")
