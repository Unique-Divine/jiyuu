import { describe, expect, test } from "bun:test";

import {
  formatSaiChatJsonLine,
  isSaiChatTitle,
  parseChatInputFile,
  parseChatInputLine,
} from "./ops";

describe("parseChatInputLine", () => {
  test("parses plain chat ID lines", () => {
    expect(parseChatInputLine("-1002094035214")).toMatchObject({
      chatId: -1002094035214,
      title: null,
    });
  });

  test("parses plain chat ID lines with trailing title text", () => {
    expect(
      parseChatInputLine("-1002094035214 Sai <> Example Chat"),
    ).toMatchObject({
      chatId: -1002094035214,
      title: "Sai <> Example Chat",
    });
  });

  test("parses JSONL chat records with chatId", () => {
    expect(
      parseChatInputLine(
        '{"chatId":-5257835121,"title":"Sai <> Adevar Labs"}',
      ),
    ).toMatchObject({
      chatId: -5257835121,
      title: "Sai <> Adevar Labs",
    });
  });

  test("parses JSONL chat records with chat_id", () => {
    expect(
      parseChatInputLine(
        '{"chat_id":-5257835121,"title":"Sai <> Adevar Labs"}',
      ),
    ).toMatchObject({
      chatId: -5257835121,
      title: "Sai <> Adevar Labs",
    });
  });

  test("ignores empty lines and comments", () => {
    expect(parseChatInputLine("")).toBeNull();
    expect(parseChatInputLine("# skip this")).toBeNull();
  });
});

describe("parseChatInputFile", () => {
  test("parses mixed plain and JSONL chat files", () => {
    const chats = parseChatInputFile(`
# reviewed by operator
-1002094035214
{"chatId":-5257835121,"title":"Sai <> Adevar Labs"}
`);

    expect(chats).toHaveLength(2);
    expect(chats.map((chat) => chat.chatId)).toEqual([
      -1002094035214,
      -5257835121,
    ]);
  });
});

describe("isSaiChatTitle", () => {
  test("matches Sai-related chat titles", () => {
    expect(isSaiChatTitle("Sai <> Adevar Labs")).toBe(true);
    expect(isSaiChatTitle("Sai.fun x Eyekon Podcasts")).toBe(true);
    expect(isSaiChatTitle("APC <> Sai.fun")).toBe(true);
  });

  test("does not match unrelated words containing sai as a substring", () => {
    expect(isSaiChatTitle("CryptoKudasaiJP")).toBe(false);
    expect(isSaiChatTitle("Nibiru Internal")).toBe(false);
  });
});

describe("formatSaiChatJsonLine", () => {
  test("formats one JSONL chat record", () => {
    expect(
      formatSaiChatJsonLine({
        chatId: -5257835121,
        title: "Sai <> Adevar Labs",
        type: "Group (Basic)",
        username: null,
      }),
    ).toBe(
      '{"chatId":-5257835121,"title":"Sai <> Adevar Labs","type":"Group (Basic)","username":null}',
    );
  });
});
