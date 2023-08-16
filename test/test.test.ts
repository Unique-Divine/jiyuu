import { describe, test, expect } from "bun:test";
import { readFileSync } from "fs";
import { RemarkablePlus } from "../src/mdtoc/remarkable";
import { toc, Toc } from "../src/mdtoc";

function strip(str: string) {
  return str.trim();
}

function read(fp: PathOrFileDescriptor) {
  return strip(readFileSync(fp, "utf8"));
}

interface WantGot {
  want: any
  got: any
}

test("new works", () => {
  const remark = new RemarkablePlus({ options: undefined });
  expect(remark).toBeDefined();
  ["ruler", "options", "renderer"].forEach((prop: string) => {
    expect(remark.core).toHaveProperty(prop);
  });
});

const REMARK = new RemarkablePlus({});

describe("plugin", function() {
  test("should work as a remarkable plugin", function() {
    function render(str: string, options: any): string {
      return REMARK.render(str, options);
    }

    const inMd: string = read("test/fixtures/strip-words.md");

    const options = {
      slugify: false,
      strip: function(str: string) {
        return "~" + str.slice(4) + "~";
      },
    };
    const got = toc(inMd, options);

    console.debug("DEBUG: %o", {
      toc: got,
      renderedToc: toc(render(inMd, options), options),
    });

    expect(got.content).toEqual(
      [
        "- [~aaa~](#foo-aaa)",
        "- [~bbb~](#bar-bbb)",
        "- [~ccc~](#baz-ccc)",
        "- [~ddd~](#fez-ddd)",
      ].join("\n"),
    );
  });
});

describe("options: custom functions:", function() {
  test("should allow a custom `strip` function to strip words from heading text:", function() {
    var actual = toc(read("test/fixtures/strip-words.md"), {
      slugify: false,
      strip: function(str: string) {
        return "~" + str.slice(4) + "~";
      },
    });

    expect(actual.content).toEqual(
      [
        "- [~aaa~](#foo-aaa)",
        "- [~bbb~](#bar-bbb)",
        "- [~ccc~](#baz-ccc)",
        "- [~ddd~](#fez-ddd)",
      ].join("\n"),
    );
  });

  test("should allow a custom slugify function to be passed:", function() {
    var actual = toc("# Some Article", {
      firsth1: true,
      slugify: function(str: string) {
        return "!" + str.replace(/[^\w]/g, "-") + "!";
      },
    });
    expect(actual.content).toEqual("- [Some Article](#!Some-Article!)");
  });

  test("should strip forward slashes in slugs", function() {
    var actual = toc("# Some/Article");
    expect(actual.content).toEqual("- [Some/Article](#somearticle)");
  });

  test("should strip backticks in slugs", function() {
    var actual = toc("# Some`Article`");
    expect(actual.content).toEqual("- [Some`Article`](#somearticle)");
  });

  test("should strip CJK punctuations in slugs", function() {
    var actual = toc("# 存在，【中文】；《标点》、符号！的标题？");
    expect(actual.content).toEqual(
      "- [存在，【中文】；《标点》、符号！的标题？](#%E5%AD%98%E5%9C%A8%E4%B8%AD%E6%96%87%E6%A0%87%E7%82%B9%E7%AC%A6%E5%8F%B7%E7%9A%84%E6%A0%87%E9%A2%98)",
    );
  });

  test("should strip & in slugs", function() {
    var actual = toc("# Foo & Bar");
    expect(actual.content).toEqual("- [Foo & Bar](#foo--bar)");
  });

  test("should escape the CJK characters in linkify", function() {
    expect(toc("# 中文").content).toEqual("- [中文](#%E4%B8%AD%E6%96%87)");
    expect(toc("# かんじ").content).toEqual(
      "- [かんじ](#%E3%81%8B%E3%82%93%E3%81%98)",
    );
    expect(toc("# 한자").content).toEqual("- [한자](#%ED%95%9C%EC%9E%90)");
  });

  test("should strip HTML tags from headings", function() {
    expect(toc("# <test>Foo").content).toEqual("- [Foo](#foo)");
    expect(toc("# <test> Foo").content).toEqual("- [Foo](#-foo)");
    expect(toc("# <test> Foo ").content).toEqual("- [Foo](#-foo)");
    expect(toc("# <div> Foo </div>").content).toEqual("- [Foo](#-foo-)");
    expect(toc("#  Foo <test>").content).toEqual("- [Foo](#foo-)");
  });

  test("should not strip HTML tags from headings when `stripHeadingTags` is false", function() {
    var opts = { stripHeadingTags: false };
    expect(toc("# <test>Foo", opts).content).toEqual("- [Foo](#testfoo)");
    expect(toc("# <test> Foo", opts).content).toEqual("- [Foo](#test-foo)");
    expect(toc("# <test> Foo ", opts).content).toEqual("- [Foo](#test-foo)");
    expect(toc("# <div> Foo </div>", opts).content).toEqual(
      "- [Foo](#div-foo-div)",
    );
    expect(toc("#  Foo <test>", opts).content).toEqual("- [Foo](#foo-test)");
  });

  test("should condense spaces in the heading text", function() {
    var actual = toc("# Some    Article");
    expect(actual.content).toEqual("- [Some Article](#some----article)");
  });

  test("should replace spaces in links with dashes", function() {
    expect(toc("# Foo - bar").content).toEqual("- [Foo - bar](#foo---bar)");
    expect(toc("# Foo- - -bar").content).toEqual(
      "- [Foo- - -bar](#foo-----bar)",
    );
    expect(toc("# Foo---bar").content).toEqual("- [Foo---bar](#foo---bar)");
    expect(toc("# Foo- - -bar").content).toEqual(
      "- [Foo- - -bar](#foo-----bar)",
    );
    expect(toc("# Foo- -   -bar").content).toEqual(
      "- [Foo- - -bar](#foo-------bar)",
    );
  });

  test("should allow a `filter` function to filter out unwanted bullets:", function() {
    var actual = toc(read("test/fixtures/filter.md"), {
      filter: function(str: string, _ele: any, _arr: any) {
        return str.indexOf("...") === -1;
      },
    });
    expect(actual.content).toEqual(
      [
        "- [AAA](#aaa)",
        "  * [a.1](#a1)",
        "    + [a.2](#a2)",
        "      - [a.3](#a3)",
        "- [BBB](#bbb)",
        "- [CCC](#ccc)",
        "- [CCC](#ccc-1)",
        "    + [bbb](#bbb)",
        "- [DDD](#ddd)",
        "- [EEE](#eee)",
        "  * [FFF](#fff)",
      ].join("\n"),
    );
  });
});

describe("toc", function() {
  test("should generate a TOC from markdown headings:", function() {
    expect(toc("# AAA\n# BBB\n# CCC\nfoo\nbar\nbaz").content).toEqual(
      ["- [AAA](#aaa)", "- [BBB](#bbb)", "- [CCC](#ccc)"].join("\n"),
    );
  });

  test("should allow duplicate headings:", function() {
    expect(toc("# AAA\n# BBB\n# BBB\n# CCC\nfoo\nbar\nbaz").content).toEqual(
      [
        "- [AAA](#aaa)",
        "- [BBB](#bbb)",
        "- [BBB](#bbb-1)",
        "- [CCC](#ccc)",
      ].join("\n"),
    );
  });

  test("should increment duplicate headings:", function() {
    expect(toc("### AAA\n### AAA\n### AAA").json[0].slug).toEqual("aaa");
    expect(toc("### AAA\n### AAA\n### AAA").json[1].slug).toEqual("aaa-1");
    expect(toc("### AAA\n### AAA\n### AAA").json[2].slug).toEqual("aaa-2");
  });

  test("should allow and ignore empty headings:", function() {
    expect(toc("#\n# \n# AAA\n# BBB\nfoo\nbar\nbaz#\n").content).toEqual(
      ["- [AAA](#aaa)", "- [BBB](#bbb)"].join("\n"),
    );
  });

  test("should handle dots, colons dashes and underscores correctly:", function() {
    expect(
      toc("# AAA:aaa\n# BBB.bbb\n# CCC-ccc\n# DDD_ddd\nfoo\nbar\nbaz").content,
    ).toEqual(
      [
        "- [AAA:aaa](#aaaaaa)",
        "- [BBB.bbb](#bbbbbb)",
        "- [CCC-ccc](#ccc-ccc)",
        "- [DDD_ddd](#ddd_ddd)",
      ].join("\n"),
    );
  });

  test("should use a different bullet for each level", function() {
    expect(toc(read("test/fixtures/levels.md")).content).toEqual(
      [
        "- [AAA](#aaa)",
        "  * [a.1](#a1)",
        "    + [a.2](#a2)",
        "      - [a.3](#a3)",
      ].join("\n"),
    );
  });

  test("should use a different bullet for each level", function() {
    expect(toc(read("test/fixtures/repeat-bullets.md")).content).toEqual(
      [
        "- [AAA](#aaa)",
        "  * [a.1](#a1)",
        "    + [a.2](#a2)",
        "      - [a.3](#a3)",
        "        * [a.4](#a4)",
      ].join("\n"),
    );
  });

  test("should handle mixed heading levels:", function() {
    expect(toc(read("test/fixtures/mixed.md")).content).toEqual(
      [
        "- [AAA](#aaa)",
        "  * [a.1](#a1)",
        "    + [a.2](#a2)",
        "      - [a.3](#a3)",
        "- [BBB](#bbb)",
        "- [CCC](#ccc)",
        "- [DDD](#ddd)",
        "- [EEE](#eee)",
        "  * [FFF](#fff)",
      ].join("\n"),
    );
  });

  test("should ignore headings in fenced code blocks.", function() {
    expect(toc(read("test/fixtures/fenced-code-blocks.md")).content).toEqual(
      [
        "- [AAA](#aaa)",
        "  * [a.1](#a1)",
        "    + [a.2](#a2)",
        "      - [a.3](#a3)",
        "- [BBB](#bbb)",
        "- [CCC](#ccc)",
        "- [DDD](#ddd)",
        "- [EEE](#eee)",
        "  * [FFF](#fff)",
      ].join("\n"),
    );
  });

  test("should allow `maxdepth` to limit heading levels:", function() {
    const got = toc("# AAA\n## BBB\n### CCC", { maxdepth: 2 }).content;
    const want = ["- [AAA](#aaa)", "  * [BBB](#bbb)"].join("\n");
    expect(got).toEqual(want);
  });

  test("should remove the first H1 when `firsth1` is false:", function() {
    expect(toc("# AAA\n## BBB\n### CCC", { firsth1: false }).content).toEqual(
      ["- [BBB](#bbb)", "  * [CCC](#ccc)"].join("\n"),
    );
  });

  test.skip("should correctly calculate `maxdepth` when `firsth1` is false:", function() {
    expect(
      toc("# AAA\n## BBB\n### CCC\n#### DDD", {
        maxdepth: 2,
        firsth1: false,
      }).content,
    ).toEqual(["- [BBB](#bbb)", "  * [CCC](#ccc)"].join("\n"));

    expect(
      toc("## BBB\n### CCC\n#### DDD", { maxdepth: 2, firsth1: false }).content,
    ).toEqual(["- [BBB](#bbb)", "  * [CCC](#ccc)"].join("\n"));
  });

  test("should allow custom bullet points to be defined:", function() {
    var actual = toc("# AAA\n# BBB\n# CCC", {
      bullets: ["?"],
    });

    expect(actual.content).toEqual(
      ["? [AAA](#aaa)", "? [BBB](#bbb)", "? [CCC](#ccc)"].join("\n"),
    );
  });

  test("should rotate bullets when there are more levels than bullets defined:", function() {
    var actual = toc("# AAA\n## BBB\n### CCC", {
      bullets: ["?"],
    });

    expect(actual.content).toEqual(
      ["? [AAA](#aaa)", "  ? [BBB](#bbb)", "    ? [CCC](#ccc)"].join("\n"),
    );
  });

  test("should rotate bullets when there are more levels than bullets defined:", function() {
    var actual = toc(read("test/fixtures/repeated-headings.md")).content;
    expect(actual).toEqual(read("test/expected/repeated-headings.md"));
  });

  test("should wrap around the bullet point array", function() {
    var actual = toc(read("test/fixtures/heading-levels.md"), {
      bullets: ["*", "-"],
    });

    expect(actual.content).toEqual(
      [
        "* [AAA](#aaa)",
        "  - [aaa](#aaa)",
        "    * [bbb](#bbb)",
        "* [BBB](#bbb)",
        "  - [aaa](#aaa-1)",
        "    * [bbb](#bbb-1)",
        "* [CCC](#ccc)",
        "  - [aaa](#aaa-2)",
        "    * [bbb](#bbb-2)",
      ].join("\n"),
    );
  });

  test("should allow custom bullet points at different depths", function() {
    var actual = toc(read("test/fixtures/heading-levels.md"), {
      bullets: ["*", "1.", "-"],
    });

    expect(actual.content).toEqual(
      [
        "* [AAA](#aaa)",
        "  1. [aaa](#aaa)",
        "    - [bbb](#bbb)",
        "* [BBB](#bbb)",
        "  1. [aaa](#aaa-1)",
        "    - [bbb](#bbb-1)",
        "* [CCC](#ccc)",
        "  1. [aaa](#aaa-2)",
        "    - [bbb](#bbb-2)",
      ].join("\n"),
    );
  });

  test("should remove diacritics from the links", function() {
    var actual = toc(read("test/fixtures/diacritics.md"));

    expect(actual.content).toEqual(
      [
        "- [Regras de formatação de código](#regras-de-formatacao-de-codigo)",
        "- [Conteúdo de autenticação côncovo](#conteudo-de-autenticacao-concovo)",
      ].join("\n"),
    );
  });

  test("should strip words from heading text, but not from urls:", function() {
    var actual = toc(read("test/fixtures/strip-words.md"), {
      strip: ["foo", "bar", "baz", "fez"],
    });

    expect(actual.content).toEqual(
      [
        "- [aaa](#foo-aaa)",
        "- [bbb](#bar-bbb)",
        "- [ccc](#baz-ccc)",
        "- [ddd](#fez-ddd)",
      ].join("\n"),
    );
  });
});

describe("toc tokens", function() {
  test("should return an object for customizing a toc:", function() {
    var actual = toc("# AAA\n## BBB\n### CCC");
    expect(actual).toBeDefined();
    expect(typeof actual).toEqual("object");
    ["content", "highest", "tokens"].forEach((prop) =>
      expect(actual).toHaveProperty(prop),
    );
  });

  test("should return the `highest` heading level in the TOC:", function() {
    expect(toc("# AAA\n## BBB\n### CCC\n#### DDD").highest).toEqual(1);
    expect(toc("## BBB\n### CCC\n#### DDD").highest).toEqual(2);
    expect(toc("### CCC\n#### DDD").highest).toEqual(3);
  });

  test("should return an array of tokens:", function() {
    expect(Array.isArray(toc("# AAA\n## BBB\n### CCC").tokens)).toBeDefined();
  });

  test("should expose the `lvl` property on headings tokens:", function() {
    var actual = toc("# AAA\n## BBB\n### CCC");
    expect(actual.tokens[1].lvl).toEqual(1);
    expect(actual.tokens[4].lvl).toEqual(2);
    expect(actual.tokens[7].lvl).toEqual(3);
  });
});

describe("json property", function() {
  test("should expose a `json` property:", function() {
    var actual = toc("# AAA\n## BBB\n## BBB\n### CCC\nfoo");
    expect(Array.isArray(actual.json)).toBeDefined();
    ["content", "lvl", "slug"].forEach((prop) =>
      expect(actual.json[0]).toHaveProperty(prop),
    );
  });

  test("should return the `content` property for a heading:", function() {
    expect(toc("# AAA\n## BBB\n### CCC\n#### DDD").json[0].content).toEqual(
      "AAA",
    );
    expect(toc("## BBB\n### CCC\n#### DDD").json[2].content).toEqual("DDD");
    expect(toc("### CCC\n#### DDD").json[0].content).toEqual("CCC");
  });
});

describe("toc.insert", function() {
  test("should retain trailing newlines in the given string", function() {
    var str = readFileSync("test/fixtures/newline.md", "utf8");
    expect(Toc.insert(str)).toEqual(
      readFileSync("test/expected/newline.md", "utf8"),
    );
  });

  test("should insert a markdown TOC beneath a `<!-- toc -->` comment.", function() {
    var str = read("test/fixtures/insert.md");
    expect(strip(Toc.insert(str))).toEqual(read("test/expected/insert.md"));
  });

  test("should replace an old TOC between `<!-- toc -->...<!-- tocstop -->` comments.", function() {
    var str = read("test/fixtures/replace-existing.md");
    expect(strip(Toc.insert(str))).toEqual(read("test/expected/replace-existing.md"));
  });

  test("should insert the toc passed on the options.", function() {
    const str = read("test/fixtures/replace-existing.md");
    const testCases: WantGot[] = [
      { want: read("test/expected/replace-existing.md"), got: strip(Toc.insert(str, { toc: toc(str).content })) },
      { want: read("test/expected/foo.md"), got: strip(Toc.insert(str, { toc: "- Foo" })) },
    ]
    testCases.forEach(tc => expect(tc.got).toEqual(tc.want))
  });

  test("should accept options", function() {
    const str = read("test/fixtures/insert.md");
    const actual = Toc.insert(str, {
      slugify: function(text: string) {
        return text.toLowerCase();
      },
    });

    expect(strip(actual)).toEqual(read("test/expected/insert-options.md"));
  });

  test("should accept no links option", function() {
    const str = read("test/fixtures/insert.md");
    const testCases: WantGot[] = [
      { want: read("test/expected/insert.md"), got: strip(Toc.insert(str, {})), },
      { want: read("test/expected/insert.md"), got: strip(Toc.insert(str, { linkify: true })) },
      { want: read("test/expected/insert-no-links.md"), got: strip(Toc.insert(str, { linkify: false })) },
    ]
    testCases.forEach(tc => expect(tc.got).toEqual(tc.want))
  });
});
