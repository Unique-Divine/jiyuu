import asyncio
from datetime import datetime, timedelta, timezone

import pytest

import session_string_generator


class _NS:
    def __init__(self, expires):
        self.expires = expires


class _FakeQR:
    def __init__(self, wait_outcomes):
        # Each entry is either an exception to raise on wait(), or None to succeed.
        self._wait_outcomes = list(wait_outcomes)
        self.url = "tg://login?token=fake"
        self.expires = datetime.now(timezone.utc) + timedelta(seconds=30)
        self.recreate_calls = 0

    def wait(self, timeout=None):
        return ("wait", self._wait_outcomes.pop(0))

    def recreate(self):
        self.recreate_calls += 1
        self.expires = datetime.now(timezone.utc) + timedelta(seconds=30)
        return ("recreate", None)


class _FakeLoop:
    def run_until_complete(self, marker):
        kind, outcome = marker
        if kind == "wait" and outcome is not None:
            raise outcome
        return None


class _FakeClient:
    def __init__(self, qr):
        self._qr = qr
        self.loop = _FakeLoop()
        self.disconnected = False
        self.sign_in_calls = []

    def qr_login(self):
        return self._qr

    def sign_in(self, password=None):
        self.sign_in_calls.append(password)

    def disconnect(self):
        self.disconnected = True


def test_client_identity_kwargs_reads_optional_device_env(monkeypatch):
    monkeypatch.setenv("TELEGRAM_DEVICE_MODEL", "tg-sai helper")
    monkeypatch.setenv("TELEGRAM_SYSTEM_VERSION", "Linux")
    monkeypatch.delenv("TELEGRAM_APP_VERSION", raising=False)

    assert session_string_generator.client_identity_kwargs() == {
        "device_model": "tg-sai helper",
        "system_version": "Linux",
    }


def test_seconds_until_expiry_handles_naive_and_aware():
    naive = datetime(2999, 1, 1)
    aware = datetime.now(timezone.utc) + timedelta(seconds=30)

    assert session_string_generator._seconds_until_expiry(_NS(naive)) > 1.0
    assert session_string_generator._seconds_until_expiry(_NS(aware)) > 1.0


def test_seconds_until_expiry_never_below_one():
    past = datetime.now(timezone.utc) - timedelta(seconds=10)

    assert session_string_generator._seconds_until_expiry(_NS(past)) == 1.0


def test_qr_login_returns_on_first_successful_scan():
    qr = _FakeQR([None])
    client = _FakeClient(qr)

    session_string_generator._qr_login(client)

    assert qr.recreate_calls == 0
    assert client.disconnected is False


def test_qr_login_refreshes_after_expiry_then_succeeds():
    qr = _FakeQR([asyncio.TimeoutError(), None])
    client = _FakeClient(qr)

    session_string_generator._qr_login(client)

    assert qr.recreate_calls == 1
    assert client.disconnected is False


def test_qr_login_gives_up_after_max_refreshes(monkeypatch):
    monkeypatch.setattr(session_string_generator, "_QR_MAX_REFRESHES", 3)
    qr = _FakeQR([asyncio.TimeoutError()] * 3)
    client = _FakeClient(qr)

    with pytest.raises(SystemExit):
        session_string_generator._qr_login(client)

    assert qr.recreate_calls == 3
    assert client.disconnected is True


def test_qr_login_uses_hidden_password_prompt_for_2fa(monkeypatch):
    qr = _FakeQR([session_string_generator.errors.SessionPasswordNeededError(request=None)])
    client = _FakeClient(qr)
    prompted = []

    monkeypatch.setattr("getpass.getpass", lambda prompt: prompted.append(prompt) or "secret")

    session_string_generator._qr_login(client)

    assert prompted == ["\nTwo-factor authentication enabled. Please enter your password: "]
    assert client.sign_in_calls == ["secret"]


def test_phone_login_uses_hidden_password_prompt_for_2fa(monkeypatch):
    class _Client:
        def __init__(self):
            self.sign_in_calls = []

        def send_code_request(self, phone):
            self.phone = phone

        def disconnect(self):
            return None

        def sign_in(self, *args, **kwargs):
            if kwargs.get("password") is not None:
                self.sign_in_calls.append(kwargs["password"])
                return None
            raise session_string_generator.errors.SessionPasswordNeededError(request=None)

    client = _Client()
    prompted = []
    entered = iter(["+10000000000", "12345"])

    monkeypatch.setattr("builtins.input", lambda _prompt="": next(entered))
    monkeypatch.setattr("getpass.getpass", lambda prompt: prompted.append(prompt) or "secret")

    session_string_generator._phone_login(client)

    assert client.phone == "+10000000000"
    assert prompted == ["Two-factor authentication enabled. Please enter your password: "]
    assert client.sign_in_calls == ["secret"]


def test_parse_args_qr_selects_qr_login(monkeypatch):
    monkeypatch.setattr("sys.argv", ["session_string_generator.py", "--qr"])

    args = session_string_generator._parse_args()

    assert args.qr is True
    assert args.phone is False


def test_parse_args_phone_selects_phone_login(monkeypatch):
    monkeypatch.setattr("sys.argv", ["session_string_generator.py", "--phone"])

    args = session_string_generator._parse_args()

    assert args.qr is False
    assert args.phone is True


def test_parse_args_accepts_session_output_and_no_env_write(monkeypatch):
    monkeypatch.setattr(
        "sys.argv",
        [
            "session_string_generator.py",
            "--qr",
            "--session-output",
            "/tmp/session.txt",
            "--no-env-write",
        ],
    )

    args = session_string_generator._parse_args()

    assert args.qr is True
    assert args.session_output == "/tmp/session.txt"
    assert args.no_env_write is True
    assert session_string_generator._is_wrapper_mode(args) is True


def test_parse_args_without_flags_keeps_interactive_login_choice(monkeypatch):
    monkeypatch.setattr("sys.argv", ["session_string_generator.py"])

    args = session_string_generator._parse_args()

    assert args.qr is False
    assert args.phone is False
    assert session_string_generator._is_wrapper_mode(args) is False


def test_parse_args_rejects_conflicting_login_modes(monkeypatch):
    monkeypatch.setattr("sys.argv", ["session_string_generator.py", "--qr", "--phone"])

    with pytest.raises(SystemExit):
        session_string_generator._parse_args()


_FAKE_SESSION = "fake-session-string-value"
_FAKE_API_HASH = "fake-api-hash-value"


class _AuthorizedClient:
    """Stands in for TelegramClient after a login that already succeeded."""

    def __init__(self, *args, **kwargs):
        self.session = object()

    def connect(self):
        return None

    def is_user_authorized(self):
        return True

    def disconnect(self):
        return None


def _run_main(monkeypatch, argv, inputs=()):
    """Runs main() with fake app credentials and an authorized fake client."""
    monkeypatch.setenv("TELEGRAM_API_ID", "123")
    monkeypatch.setenv("TELEGRAM_API_HASH", _FAKE_API_HASH)
    monkeypatch.setattr("sys.argv", ["session_string_generator.py", *argv])
    monkeypatch.setattr(session_string_generator, "TelegramClient", _AuthorizedClient)
    monkeypatch.setattr(
        session_string_generator.StringSession, "save", staticmethod(lambda _session: _FAKE_SESSION)
    )
    prompts = []
    answers = iter(inputs)

    def fake_input(prompt=""):
        prompts.append(prompt)
        return next(answers)

    monkeypatch.setattr("builtins.input", fake_input)
    session_string_generator.main()
    return prompts


@pytest.mark.parametrize(
    "extra_args",
    [["--no-env-write"], []],
    ids=["with-no-env-write", "session-output-only"],
)
def test_wrapper_mode_writes_session_only_to_output_file(tmp_path, monkeypatch, capsys, extra_args):
    output = tmp_path / "session.txt"

    prompts = _run_main(monkeypatch, ["--qr", "--session-output", str(output), *extra_args])

    captured = capsys.readouterr()
    assert prompts == []
    assert output.read_text(encoding="utf-8") == f"{_FAKE_SESSION}\n"
    assert "Session captured by tg-sai" in captured.out
    for value in (_FAKE_SESSION, _FAKE_API_HASH):
        assert value not in captured.out
        assert value not in captured.err


def test_standalone_guidance_uses_tg_sai_without_api_hash(monkeypatch, capsys):
    prompts = _run_main(monkeypatch, ["--qr", "--no-env-write"], inputs=[""])

    captured = capsys.readouterr()
    assert len(prompts) == 1 and prompts[0].startswith("Account label")
    assert _FAKE_SESSION in captured.out
    assert _FAKE_API_HASH not in captured.out
    assert "tg-sai profile add --api-id 123 --api-hash '<api hash>'" in captured.out
    assert "just tg-sai" not in captured.out


def test_missing_app_credentials_names_every_source(monkeypatch, capsys):
    monkeypatch.delenv("TELEGRAM_API_ID", raising=False)
    monkeypatch.delenv("TELEGRAM_API_HASH", raising=False)
    monkeypatch.setattr("sys.argv", ["session_string_generator.py", "--qr", "--session-output", "/unused"])

    with pytest.raises(SystemExit):
        session_string_generator.main()

    out = capsys.readouterr().out
    for source in ("environment", ".env file", "--api-id", "~/.tg-sai/.env"):
        assert source in out
