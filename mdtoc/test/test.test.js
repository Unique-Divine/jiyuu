"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
var bun_test_1 = require("bun:test"); // eslint-disable-line import/no-unresolved
var fs_1 = require("fs");
var remarkable_1 = require("../src/mdtoc/remarkable");
var mdtoc_1 = require("../src/mdtoc");
function strip(str) {
    return str.trim();
}
function read(fp) {
    return strip((0, fs_1.readFileSync)(fp, "utf8"));
}
(0, bun_test_1.test)("new works", function () {
    var remark = new remarkable_1.RemarkablePlus({ options: undefined });
    (0, bun_test_1.expect)(remark).toBeDefined();
    ["ruler", "options", "renderer"].forEach(function (prop) {
        (0, bun_test_1.expect)(remark.core).toHaveProperty(prop);
    });
});
var REMARK = new remarkable_1.RemarkablePlus({});
(0, bun_test_1.describe)("plugin", function () {
    (0, bun_test_1.test)("should work as a remarkable plugin", function () {
        function render(str, options) {
            return REMARK.render(str, options);
        }
        var inMd = read("test/fixtures/strip-words.md");
        var options = {
            slugify: false,
            strip: function (str) {
                return "~".concat(str.slice(4), "~");
            },
        };
        var got = (0, mdtoc_1.toc)(inMd, options);
        (0, bun_test_1.expect)(got.content).toEqual([
            "- [~aaa~](#foo-aaa)",
            "- [~bbb~](#bar-bbb)",
            "- [~ccc~](#baz-ccc)",
            "- [~ddd~](#fez-ddd)",
        ].join("\n"));
    });
});
(0, bun_test_1.describe)("options: custom functions:", function () {
    (0, bun_test_1.test)("should allow a custom `strip` function to strip words from heading text:", function () {
        var actual = (0, mdtoc_1.toc)(read("test/fixtures/strip-words.md"), {
            slugify: false,
            strip: function (str) {
                return "~".concat(str.slice(4), "~");
            },
        });
        (0, bun_test_1.expect)(actual.content).toEqual([
            "- [~aaa~](#foo-aaa)",
            "- [~bbb~](#bar-bbb)",
            "- [~ccc~](#baz-ccc)",
            "- [~ddd~](#fez-ddd)",
        ].join("\n"));
    });
    (0, bun_test_1.test)("should allow a custom slugify function to be passed:", function () {
        var actual = (0, mdtoc_1.toc)("# Some Article", {
            firsth1: true,
            slugify: function (str) {
                return "!".concat(str.replace(/[^\w]/g, "-"), "!");
            },
        });
        (0, bun_test_1.expect)(actual.content).toEqual("- [Some Article](#!Some-Article!)");
    });
    (0, bun_test_1.test)("should strip forward slashes in slugs", function () {
        var actual = (0, mdtoc_1.toc)("# Some/Article");
        (0, bun_test_1.expect)(actual.content).toEqual("- [Some/Article](#somearticle)");
    });
    (0, bun_test_1.test)("should strip backticks in slugs", function () {
        var actual = (0, mdtoc_1.toc)("# Some`Article`");
        (0, bun_test_1.expect)(actual.content).toEqual("- [Some`Article`](#somearticle)");
    });
    (0, bun_test_1.test)("should strip CJK punctuations in slugs", function () {
        var actual = (0, mdtoc_1.toc)("# 存在，【中文】；《标点》、符号！的标题？");
        (0, bun_test_1.expect)(actual.content).toEqual("- [存在，【中文】；《标点》、符号！的标题？](#%E5%AD%98%E5%9C%A8%E4%B8%AD%E6%96%87%E6%A0%87%E7%82%B9%E7%AC%A6%E5%8F%B7%E7%9A%84%E6%A0%87%E9%A2%98)");
    });
    (0, bun_test_1.test)("should strip & in slugs", function () {
        var actual = (0, mdtoc_1.toc)("# Foo & Bar");
        (0, bun_test_1.expect)(actual.content).toEqual("- [Foo & Bar](#foo--bar)");
    });
    (0, bun_test_1.test)("should escape the CJK characters in linkify", function () {
        (0, bun_test_1.expect)((0, mdtoc_1.toc)("# 中文").content).toEqual("- [中文](#%E4%B8%AD%E6%96%87)");
        (0, bun_test_1.expect)((0, mdtoc_1.toc)("# かんじ").content).toEqual("- [かんじ](#%E3%81%8B%E3%82%93%E3%81%98)");
        (0, bun_test_1.expect)((0, mdtoc_1.toc)("# 한자").content).toEqual("- [한자](#%ED%95%9C%EC%9E%90)");
    });
    (0, bun_test_1.test)("should strip HTML tags from headings", function () {
        (0, bun_test_1.expect)((0, mdtoc_1.toc)("# <test>Foo").content).toEqual("- [Foo](#foo)");
        (0, bun_test_1.expect)((0, mdtoc_1.toc)("# <test> Foo").content).toEqual("- [Foo](#-foo)");
        (0, bun_test_1.expect)((0, mdtoc_1.toc)("# <test> Foo ").content).toEqual("- [Foo](#-foo)");
        (0, bun_test_1.expect)((0, mdtoc_1.toc)("# <div> Foo </div>").content).toEqual("- [Foo](#-foo-)");
        (0, bun_test_1.expect)((0, mdtoc_1.toc)("#  Foo <test>").content).toEqual("- [Foo](#foo-)");
    });
    (0, bun_test_1.test)("should not strip HTML tags from headings when `stripHeadingTags` is false", function () {
        var opts = { stripHeadingTags: false };
        (0, bun_test_1.expect)((0, mdtoc_1.toc)("# <test>Foo", opts).content).toEqual("- [Foo](#testfoo)");
        (0, bun_test_1.expect)((0, mdtoc_1.toc)("# <test> Foo", opts).content).toEqual("- [Foo](#test-foo)");
        (0, bun_test_1.expect)((0, mdtoc_1.toc)("# <test> Foo ", opts).content).toEqual("- [Foo](#test-foo)");
        (0, bun_test_1.expect)((0, mdtoc_1.toc)("# <div> Foo </div>", opts).content).toEqual("- [Foo](#div-foo-div)");
        (0, bun_test_1.expect)((0, mdtoc_1.toc)("#  Foo <test>", opts).content).toEqual("- [Foo](#foo-test)");
    });
    (0, bun_test_1.test)("should condense spaces in the heading text", function () {
        var actual = (0, mdtoc_1.toc)("# Some    Article");
        (0, bun_test_1.expect)(actual.content).toEqual("- [Some Article](#some----article)");
    });
    (0, bun_test_1.test)("should replace spaces in links with dashes", function () {
        (0, bun_test_1.expect)((0, mdtoc_1.toc)("# Foo - bar").content).toEqual("- [Foo - bar](#foo---bar)");
        (0, bun_test_1.expect)((0, mdtoc_1.toc)("# Foo- - -bar").content).toEqual("- [Foo- - -bar](#foo-----bar)");
        (0, bun_test_1.expect)((0, mdtoc_1.toc)("# Foo---bar").content).toEqual("- [Foo---bar](#foo---bar)");
        (0, bun_test_1.expect)((0, mdtoc_1.toc)("# Foo- - -bar").content).toEqual("- [Foo- - -bar](#foo-----bar)");
        (0, bun_test_1.expect)((0, mdtoc_1.toc)("# Foo- -   -bar").content).toEqual("- [Foo- - -bar](#foo-------bar)");
    });
    (0, bun_test_1.test)("should allow a `filter` function to filter out unwanted bullets:", function () {
        var actual = (0, mdtoc_1.toc)(read("test/fixtures/filter.md"), {
            filter: function (str, _ele, _arr) {
                // When first appearance of substring "..." occurs at position -1
                // TODO: Q: What does this mean?
                return str.indexOf("...") === -1 ? str : "";
            },
        });
        (0, bun_test_1.expect)(actual.content).toEqual([
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
        ].join("\n"));
    });
});
(0, bun_test_1.describe)("toc", function () {
    (0, bun_test_1.test)("should generate a TOC from markdown headings:", function () {
        (0, bun_test_1.expect)((0, mdtoc_1.toc)("# AAA\n# BBB\n# CCC\nfoo\nbar\nbaz").content).toEqual(["- [AAA](#aaa)", "- [BBB](#bbb)", "- [CCC](#ccc)"].join("\n"));
    });
    (0, bun_test_1.test)("should allow duplicate headings:", function () {
        (0, bun_test_1.expect)((0, mdtoc_1.toc)("# AAA\n# BBB\n# BBB\n# CCC\nfoo\nbar\nbaz").content).toEqual([
            "- [AAA](#aaa)",
            "- [BBB](#bbb)",
            "- [BBB](#bbb-1)",
            "- [CCC](#ccc)",
        ].join("\n"));
    });
    (0, bun_test_1.test)("should increment duplicate headings:", function () {
        (0, bun_test_1.expect)((0, mdtoc_1.toc)("### AAA\n### AAA\n### AAA").json[0].slug).toEqual("aaa");
        (0, bun_test_1.expect)((0, mdtoc_1.toc)("### AAA\n### AAA\n### AAA").json[1].slug).toEqual("aaa-1");
        (0, bun_test_1.expect)((0, mdtoc_1.toc)("### AAA\n### AAA\n### AAA").json[2].slug).toEqual("aaa-2");
    });
    (0, bun_test_1.test)("should allow and ignore empty headings:", function () {
        (0, bun_test_1.expect)((0, mdtoc_1.toc)("#\n# \n# AAA\n# BBB\nfoo\nbar\nbaz#\n").content).toEqual(["- [AAA](#aaa)", "- [BBB](#bbb)"].join("\n"));
    });
    (0, bun_test_1.test)("should handle dots, colons dashes and underscores correctly:", function () {
        (0, bun_test_1.expect)((0, mdtoc_1.toc)("# AAA:aaa\n# BBB.bbb\n# CCC-ccc\n# DDD_ddd\nfoo\nbar\nbaz").content).toEqual([
            "- [AAA:aaa](#aaaaaa)",
            "- [BBB.bbb](#bbbbbb)",
            "- [CCC-ccc](#ccc-ccc)",
            "- [DDD_ddd](#ddd_ddd)",
        ].join("\n"));
    });
    (0, bun_test_1.test)("should use a different bullet for each level", function () {
        (0, bun_test_1.expect)((0, mdtoc_1.toc)(read("test/fixtures/levels.md")).content).toEqual([
            "- [AAA](#aaa)",
            "  * [a.1](#a1)",
            "    + [a.2](#a2)",
            "      - [a.3](#a3)",
        ].join("\n"));
    });
    (0, bun_test_1.test)("should use a different bullet for each level", function () {
        (0, bun_test_1.expect)((0, mdtoc_1.toc)(read("test/fixtures/repeat-bullets.md")).content).toEqual([
            "- [AAA](#aaa)",
            "  * [a.1](#a1)",
            "    + [a.2](#a2)",
            "      - [a.3](#a3)",
            "        * [a.4](#a4)",
        ].join("\n"));
    });
    (0, bun_test_1.test)("should handle mixed heading levels:", function () {
        (0, bun_test_1.expect)((0, mdtoc_1.toc)(read("test/fixtures/mixed.md")).content).toEqual([
            "- [AAA](#aaa)",
            "  * [a.1](#a1)",
            "    + [a.2](#a2)",
            "      - [a.3](#a3)",
            "- [BBB](#bbb)",
            "- [CCC](#ccc)",
            "- [DDD](#ddd)",
            "- [EEE](#eee)",
            "  * [FFF](#fff)",
        ].join("\n"));
    });
    (0, bun_test_1.test)("should ignore headings in fenced code blocks.", function () {
        (0, bun_test_1.expect)((0, mdtoc_1.toc)(read("test/fixtures/fenced-code-blocks.md")).content).toEqual([
            "- [AAA](#aaa)",
            "  * [a.1](#a1)",
            "    + [a.2](#a2)",
            "      - [a.3](#a3)",
            "- [BBB](#bbb)",
            "- [CCC](#ccc)",
            "- [DDD](#ddd)",
            "- [EEE](#eee)",
            "  * [FFF](#fff)",
        ].join("\n"));
    });
    (0, bun_test_1.test)("should allow `maxdepth` to limit heading levels:", function () {
        var got = (0, mdtoc_1.toc)("# AAA\n## BBB\n### CCC", { maxdepth: 2 }).content;
        var want = ["- [AAA](#aaa)", "  * [BBB](#bbb)"].join("\n");
        (0, bun_test_1.expect)(got).toEqual(want);
    });
    // TODO: implement the firsth1 removal functionality. Prefer removal of the
    // first h1 by default.
    bun_test_1.test.skip("should remove the first H1 when `firsth1` is false:", function () {
        console.debug("DEBUG TC");
        (0, bun_test_1.expect)((0, mdtoc_1.toc)("# AAA\n## BBB\n### CCC", { firsth1: false }).content).toEqual(["- [BBB](#bbb)", "  * [CCC](#ccc)"].join("\n"));
    });
    // TODO: implement the firsth1 removal functionality. Prefer removal of the
    // first h1 by default.
    bun_test_1.test.skip("should correctly calculate `maxdepth` when `firsth1` is false:", function () {
        (0, bun_test_1.expect)((0, mdtoc_1.toc)("# AAA\n## BBB\n### CCC\n#### DDD", {
            maxdepth: 2,
            firsth1: false,
        }).content).toEqual(["- [BBB](#bbb)", "  * [CCC](#ccc)"].join("\n"));
        (0, bun_test_1.expect)((0, mdtoc_1.toc)("## BBB\n### CCC\n#### DDD", { maxdepth: 2, firsth1: false }).content).toEqual(["- [BBB](#bbb)", "  * [CCC](#ccc)"].join("\n"));
    });
    (0, bun_test_1.test)("should allow custom bullet points to be defined:", function () {
        var actual = (0, mdtoc_1.toc)("# AAA\n# BBB\n# CCC", {
            bullets: ["?"],
        });
        (0, bun_test_1.expect)(actual.content).toEqual(["? [AAA](#aaa)", "? [BBB](#bbb)", "? [CCC](#ccc)"].join("\n"));
    });
    (0, bun_test_1.test)("should rotate bullets when there are more levels than bullets defined:", function () {
        var actual = (0, mdtoc_1.toc)("# AAA\n## BBB\n### CCC", {
            bullets: ["?"],
        });
        (0, bun_test_1.expect)(actual.content).toEqual(["? [AAA](#aaa)", "  ? [BBB](#bbb)", "    ? [CCC](#ccc)"].join("\n"));
    });
    (0, bun_test_1.test)("should rotate bullets when there are more levels than bullets defined:", function () {
        var actual = (0, mdtoc_1.toc)(read("test/fixtures/repeated-headings.md")).content;
        (0, bun_test_1.expect)(actual).toEqual(read("test/expected/repeated-headings.md"));
    });
    (0, bun_test_1.test)("should wrap around the bullet point array", function () {
        var actual = (0, mdtoc_1.toc)(read("test/fixtures/heading-levels.md"), {
            bullets: ["*", "-"],
        });
        (0, bun_test_1.expect)(actual.content).toEqual([
            "* [AAA](#aaa)",
            "  - [aaa](#aaa)",
            "    * [bbb](#bbb)",
            "* [BBB](#bbb)",
            "  - [aaa](#aaa-1)",
            "    * [bbb](#bbb-1)",
            "* [CCC](#ccc)",
            "  - [aaa](#aaa-2)",
            "    * [bbb](#bbb-2)",
        ].join("\n"));
    });
    (0, bun_test_1.test)("should allow custom bullet points at different depths", function () {
        var actual = (0, mdtoc_1.toc)(read("test/fixtures/heading-levels.md"), {
            bullets: ["*", "1.", "-"],
        });
        (0, bun_test_1.expect)(actual.content).toEqual([
            "* [AAA](#aaa)",
            "  1. [aaa](#aaa)",
            "    - [bbb](#bbb)",
            "* [BBB](#bbb)",
            "  1. [aaa](#aaa-1)",
            "    - [bbb](#bbb-1)",
            "* [CCC](#ccc)",
            "  1. [aaa](#aaa-2)",
            "    - [bbb](#bbb-2)",
        ].join("\n"));
    });
    (0, bun_test_1.test)("should remove diacritics from the links", function () {
        var actual = (0, mdtoc_1.toc)(read("test/fixtures/diacritics.md"));
        (0, bun_test_1.expect)(actual.content).toEqual([
            "- [Regras de formatação de código](#regras-de-formatacao-de-codigo)",
            "- [Conteúdo de autenticação côncovo](#conteudo-de-autenticacao-concovo)",
        ].join("\n"));
    });
    (0, bun_test_1.test)("should strip words from heading text, but not from urls:", function () {
        var actual = (0, mdtoc_1.toc)(read("test/fixtures/strip-words.md"), {
            strip: ["foo", "bar", "baz", "fez"],
        });
        (0, bun_test_1.expect)(actual.content).toEqual([
            "- [aaa](#foo-aaa)",
            "- [bbb](#bar-bbb)",
            "- [ccc](#baz-ccc)",
            "- [ddd](#fez-ddd)",
        ].join("\n"));
    });
});
(0, bun_test_1.describe)("toc tokens", function () {
    (0, bun_test_1.test)("should return an object for customizing a toc:", function () {
        var actual = (0, mdtoc_1.toc)("# AAA\n## BBB\n### CCC");
        (0, bun_test_1.expect)(actual).toBeDefined();
        (0, bun_test_1.expect)(typeof actual).toEqual("object");
        ["content", "highest", "tokens"].forEach(function (prop) {
            return (0, bun_test_1.expect)(actual).toHaveProperty(prop);
        });
    });
    (0, bun_test_1.test)("should return the `highest` heading level in the TOC:", function () {
        (0, bun_test_1.expect)((0, mdtoc_1.toc)("# AAA\n## BBB\n### CCC\n#### DDD").highest).toEqual(1);
        (0, bun_test_1.expect)((0, mdtoc_1.toc)("## BBB\n### CCC\n#### DDD").highest).toEqual(2);
        (0, bun_test_1.expect)((0, mdtoc_1.toc)("### CCC\n#### DDD").highest).toEqual(3);
    });
    (0, bun_test_1.test)("should return an array of tokens:", function () {
        (0, bun_test_1.expect)(Array.isArray((0, mdtoc_1.toc)("# AAA\n## BBB\n### CCC").tokens)).toBeDefined();
    });
    (0, bun_test_1.test)("should expose the `lvl` property on headings tokens:", function () {
        var actual = (0, mdtoc_1.toc)("# AAA\n## BBB\n### CCC");
        (0, bun_test_1.expect)(actual.tokens[1].lvl).toEqual(1);
        (0, bun_test_1.expect)(actual.tokens[4].lvl).toEqual(2);
        (0, bun_test_1.expect)(actual.tokens[7].lvl).toEqual(3);
    });
});
(0, bun_test_1.describe)("json property", function () {
    (0, bun_test_1.test)("should expose a `json` property:", function () {
        var actual = (0, mdtoc_1.toc)("# AAA\n## BBB\n## BBB\n### CCC\nfoo");
        (0, bun_test_1.expect)(Array.isArray(actual.json)).toBeDefined();
        ["content", "lvl", "slug"].forEach(function (prop) {
            return (0, bun_test_1.expect)(actual.json[0]).toHaveProperty(prop);
        });
    });
    (0, bun_test_1.test)("should return the `content` property for a heading:", function () {
        (0, bun_test_1.expect)((0, mdtoc_1.toc)("# AAA\n## BBB\n### CCC\n#### DDD").json[0].content).toEqual("AAA");
        (0, bun_test_1.expect)((0, mdtoc_1.toc)("## BBB\n### CCC\n#### DDD").json[2].content).toEqual("DDD");
        (0, bun_test_1.expect)((0, mdtoc_1.toc)("### CCC\n#### DDD").json[0].content).toEqual("CCC");
    });
});
(0, bun_test_1.describe)("toc.insert", function () {
    (0, bun_test_1.test)("should retain trailing newlines in the given string", function () {
        var str = (0, fs_1.readFileSync)("test/fixtures/newline.md", "utf8");
        (0, bun_test_1.expect)(mdtoc_1.Toc.insert(str)).toEqual((0, fs_1.readFileSync)("test/expected/newline.md", "utf8"));
    });
    (0, bun_test_1.test)("should insert a markdown TOC beneath a `<!-- toc -->` comment.", function () {
        var str = read("test/fixtures/insert.md");
        var got = strip(mdtoc_1.Toc.insert(str));
        var want = read("test/expected/insert.md");
        (0, bun_test_1.expect)(got).toEqual(want);
    });
    (0, bun_test_1.test)("should replace an old TOC between `<!-- toc -->...<!-- tocstop -->` comments.", function () {
        var md = read("test/fixtures/replace-existing.md");
        var _a = [
            strip(mdtoc_1.Toc.insert(md)),
            read("test/expected/replace-existing.md"),
        ], got = _a[0], want = _a[1];
        (0, bun_test_1.expect)(got).toEqual(want);
    });
    (0, bun_test_1.test)("should insert the toc passed on the options.", function () {
        var md = read("test/fixtures/replace-existing.md");
        var testCases = [
            {
                want: read("test/expected/replace-existing.md"),
                got: strip(mdtoc_1.Toc.insert(md, { toc: (0, mdtoc_1.toc)(md).content })),
            },
            {
                want: read("test/expected/foo.md"),
                got: strip(mdtoc_1.Toc.insert(md, { toc: "- Foo" })),
            },
        ];
        testCases.forEach(function (tc) { return (0, bun_test_1.expect)(tc.got).toEqual(tc.want); });
    });
    (0, bun_test_1.test)("should accept options", function () {
        var md = read("test/fixtures/insert.md");
        var actual = mdtoc_1.Toc.insert(md, {
            slugify: function (text) {
                return text.toLowerCase();
            },
        });
        (0, bun_test_1.expect)(strip(actual)).toEqual(read("test/expected/insert-options.md"));
    });
    (0, bun_test_1.test)("should accept no links option", function () {
        var md = read("test/fixtures/insert.md");
        var testCases = [
            { want: read("test/expected/insert.md"), got: strip(mdtoc_1.Toc.insert(md, {})) },
            {
                want: read("test/expected/insert.md"),
                got: strip(mdtoc_1.Toc.insert(md, { linkify: true })),
            },
            {
                want: read("test/expected/insert-no-links.md"),
                got: strip(mdtoc_1.Toc.insert(md, { linkify: false })),
            },
        ];
        testCases.forEach(function (tc) { return (0, bun_test_1.expect)(tc.got).toEqual(tc.want); });
    });
});
