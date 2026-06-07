#!/usr/bin/env python3
"""
Telegram session string generator for tg-sai.

This script generates a Telethon session string that can be saved into a local
tg-sai profile. It is a standalone copy of the telegram-mcp helper with the
package-local imports removed.

Usage:
    uv run session_string_generator.py
    uv run session_string_generator.py --qr
"""

import argparse
import asyncio
import getpass
import io
import os
import sys
from datetime import datetime, timezone

from dotenv import load_dotenv
from telethon import errors
from telethon.sessions import StringSession
from telethon.sync import TelegramClient

# How many times the QR code is regenerated after expiry before giving up.
_QR_MAX_REFRESHES = 10

load_dotenv()


def client_identity_kwargs() -> dict:
    """Return optional Telegram device identity kwargs from the environment."""
    env_map = {
        "device_model": "TELEGRAM_DEVICE_MODEL",
        "system_version": "TELEGRAM_SYSTEM_VERSION",
        "app_version": "TELEGRAM_APP_VERSION",
    }
    return {
        kwarg: os.environ[env_var]
        for kwarg, env_var in env_map.items()
        if os.environ.get(env_var)
    }


def _parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Generate a Telegram session string for tg-sai.")
    login_group = parser.add_mutually_exclusive_group()
    login_group.add_argument(
        "--qr",
        action="store_true",
        help="Use Telegram QR login without prompting for a login method.",
    )
    login_group.add_argument(
        "--phone",
        action="store_true",
        help="Use phone number + verification code login without prompting for a login method.",
    )
    parser.add_argument(
        "--session-output",
        help="Write the generated session string to this file for wrapper tools.",
    )
    parser.add_argument(
        "--no-env-write",
        action="store_true",
        help="Do not prompt to update .env after printing the session string.",
    )
    return parser.parse_args()


def _render_qr(qr) -> None:
    import qrcode

    print("\n----- QR Code Login -----\n")

    qr_obj = qrcode.QRCode(border=1)
    qr_obj.add_data(qr.url)
    qr_obj.make(fit=True)
    f = io.StringIO()
    qr_obj.print_ascii(out=f, invert=True)
    print(f.getvalue())

    print("Scan the QR code above with your Telegram app:")
    print("  Open Telegram > Settings > Devices > Link Desktop Device\n")
    print(f"Or open this link on a device where you're logged in:\n  {qr.url}\n")
    print(f"Expires at: {qr.expires.strftime('%H:%M:%S')}")
    print("Waiting for you to scan...")


def _seconds_until_expiry(qr) -> float:
    """Seconds left before this QR token expires, with a small safety margin."""
    expires = qr.expires
    if expires.tzinfo is None:
        expires = expires.replace(tzinfo=timezone.utc)
    remaining = (expires - datetime.now(timezone.utc)).total_seconds()
    return max(1.0, remaining - 1.0)


def _qr_login(client: TelegramClient) -> None:
    qr = client.qr_login()
    _render_qr(qr)

    for _ in range(_QR_MAX_REFRESHES):
        try:
            client.loop.run_until_complete(qr.wait(timeout=_seconds_until_expiry(qr)))
            return
        except asyncio.TimeoutError:
            client.loop.run_until_complete(qr.recreate())
            print("\nQR code expired, here is a fresh one.")
            _render_qr(qr)
        except errors.SessionPasswordNeededError:
            pw = getpass.getpass(
                "\nTwo-factor authentication enabled. Please enter your password: "
            )
            client.sign_in(password=pw)
            return

    print("\nQR code expired too many times. Please run the generator again.")
    client.disconnect()
    sys.exit(1)


def _phone_login(client: TelegramClient) -> None:
    phone = input("Please enter your phone (or bot token): ")

    try:
        client.send_code_request(phone)
    except errors.FloodWaitError as e:
        print(f"\nFlood wait error; you must wait {e.seconds} seconds before trying again.")
        client.disconnect()
        sys.exit(1)
    except errors.PhoneNumberInvalidError:
        print("\nThe phone number is invalid.")
        client.disconnect()
        sys.exit(1)
    except Exception as e:
        print(f"\nError sending code: {e}")
        client.disconnect()
        sys.exit(1)

    code = input("\nPlease enter the code you received: ")
    try:
        client.sign_in(phone, code)
    except errors.SessionPasswordNeededError:
        pw = getpass.getpass("Two-factor authentication enabled. Please enter your password: ")
        client.sign_in(password=pw)


def main() -> None:
    args = _parse_args()

    api_id = os.getenv("TELEGRAM_API_ID")
    api_hash = os.getenv("TELEGRAM_API_HASH")

    if not api_id or not api_hash:
        print("Error: TELEGRAM_API_ID and TELEGRAM_API_HASH must be set in .env file")
        print("Create an .env file with your credentials from https://my.telegram.org/apps")
        sys.exit(1)

    try:
        api_id_int = int(api_id)
    except ValueError:
        print("Error: TELEGRAM_API_ID must be an integer")
        sys.exit(1)

    print("\n----- Telegram Session String Generator -----\n")
    print("This script will generate a session string for your Telegram account.")
    print("The generated session string can be added to your tg-sai profile.")
    print("\nYour credentials are only used for local Telegram authentication.\n")

    label = (
        input("Account label (optional, e.g. 'work', 'personal'; leave empty for default): ")
        .strip()
        .lower()
    )

    if args.qr:
        method = "1"
    elif args.phone:
        method = "2"
    else:
        print("\nChoose login method:")
        print("  1) QR code login (recommended -- scan from your Telegram app)")
        print("  2) Phone number + verification code")
        method = input("\nEnter 1 or 2 [default: 1]: ").strip() or "1"

    try:
        client = TelegramClient(
            StringSession(),
            api_id_int,
            api_hash,
            **client_identity_kwargs(),
        )
        client.connect()

        if not client.is_user_authorized():
            if method == "1":
                _qr_login(client)
            else:
                _phone_login(client)

        session_string = StringSession.save(client.session)

        if label:
            env_var = f"TELEGRAM_SESSION_STRING_{label.upper()}"
        else:
            env_var = "TELEGRAM_SESSION_STRING"

        print("\nAuthentication successful!")
        print("\n----- Your Session String -----")
        print(f"\n{session_string}\n")
        print("Save this in tg-sai with:")
        print(
            "just tg-sai profile add "
            "--name <name> "
            f"--api-id {api_id_int} "
            f"--api-hash {api_hash} "
            "--session-string '<session string>'"
        )
        print("\nOr add this to an .env file as:")
        print(f"{env_var}={session_string}")
        print("\nIMPORTANT: Keep this string private and never share it with anyone!")

        if args.session_output:
            with open(args.session_output, "w", encoding="utf-8") as file:
                file.write(f"{session_string}\n")

        if args.no_env_write:
            choice = "n"
        else:
            choice = input(
                "\nWould you like to automatically update your .env file with this session string? (y/N): "
            )
        if choice.lower() == "y":
            try:
                with open(".env", "r", encoding="utf-8") as file:
                    env_contents = file.readlines()

                session_string_line_found = False
                for i, line in enumerate(env_contents):
                    if line.startswith(f"{env_var}="):
                        env_contents[i] = f"{env_var}={session_string}\n"
                        session_string_line_found = True
                        break

                if not session_string_line_found:
                    env_contents.append(f"{env_var}={session_string}\n")

                with open(".env", "w", encoding="utf-8") as file:
                    file.writelines(env_contents)

                print("\n.env file updated successfully!")
            except Exception as e:
                print(f"\nError updating .env file: {e}")
                print("Please manually add the session string to your .env file.")

        client.disconnect()

    except Exception as e:
        print(f"\nError: {e}")
        print("Failed to generate session string. Please try again.")
        sys.exit(1)


if __name__ == "__main__":
    main()
