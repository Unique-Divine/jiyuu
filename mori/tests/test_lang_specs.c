/*
 * test_lang_specs.c — The Rust language-spec tables against the C reference.
 *
 * lang_specs.rs serves cbm_lang_spec, cbm_ts_language and
 * cbm_conditional_block_types. Test-seam builds still compile the C tables
 * in lang_specs.c under cbm_c_reference_*; these tests walk every language
 * (and the out-of-range values around them) through every accessor and
 * require the two to agree on every field, every kind string and every NULL.
 */
#include "test_framework.h"
#include "cbm.h"
#include "lang_specs.h"

#include <stdio.h>
#include <string.h>

#ifdef CBM_ENABLE_TEST_SEAMS

/* First language past the range on each side, plus far-out values. */
static const int k_probe_extra[] = {-1000, -2, -1, CBM_LANG_COUNT, CBM_LANG_COUNT + 1, 100000};

/* Compares two NULL-terminated string lists; a NULL list matches only NULL.
 * Reports the first difference with its language and field. */
static int kinds_equal(const char *const *rust, const char *const *c, int lang,
                       const char *field) {
    if (!rust || !c) {
        if (rust != c) {
            fprintf(stderr, "  lang %d %s: rust %s, c %s\n", lang, field, rust ? "list" : "NULL",
                    c ? "list" : "NULL");
            return 0;
        }
        return 1;
    }
    int i = 0;
    for (; rust[i] && c[i]; i++) {
        if (strcmp(rust[i], c[i]) != 0) {
            fprintf(stderr, "  lang %d %s[%d]: rust \"%s\", c \"%s\"\n", lang, field, i, rust[i],
                    c[i]);
            return 0;
        }
    }
    if (rust[i] || c[i]) {
        fprintf(stderr, "  lang %d %s: length differs at %d\n", lang, field, i);
        return 0;
    }
    return 1;
}

static int str_equal(const char *rust, const char *c, int lang, const char *field) {
    if (!rust || !c) {
        if (rust != c) {
            fprintf(stderr, "  lang %d %s: rust %s, c %s\n", lang, field, rust ? rust : "NULL",
                    c ? c : "NULL");
            return 0;
        }
        return 1;
    }
    if (strcmp(rust, c) != 0) {
        fprintf(stderr, "  lang %d %s: rust \"%s\", c \"%s\"\n", lang, field, rust, c);
        return 0;
    }
    return 1;
}

static int embedded_equal(const CBMEmbeddedLangSpec *rust, const CBMEmbeddedLangSpec *c,
                          int lang) {
    if (!rust || !c) {
        if (rust != c) {
            fprintf(stderr, "  lang %d embedded_imports: one side NULL\n", lang);
            return 0;
        }
        return 1;
    }
    int i = 0;
    for (;; i++) {
        if (!str_equal(rust[i].script_node_type, c[i].script_node_type, lang,
                       "embedded_imports.script_node_type") ||
            !str_equal(rust[i].content_node_type, c[i].content_node_type, lang,
                       "embedded_imports.content_node_type")) {
            return 0;
        }
        if (rust[i].embedded_language != c[i].embedded_language) {
            fprintf(stderr, "  lang %d embedded_imports[%d].embedded_language: %d vs %d\n", lang,
                    i, (int)rust[i].embedded_language, (int)c[i].embedded_language);
            return 0;
        }
        if (!c[i].script_node_type) {
            return 1;
        }
    }
}

/* Every field of the two rows for one language. */
static int spec_equal(int lang) {
    const CBMLangSpec *rust = cbm_lang_spec((CBMLanguage)lang);
    const CBMLangSpec *c = cbm_c_reference_lang_spec((CBMLanguage)lang);
    if (!rust || !c) {
        if (rust != c) {
            fprintf(stderr, "  lang %d: rust spec %s, c spec %s\n", lang, rust ? "set" : "NULL",
                    c ? "set" : "NULL");
            return 0;
        }
        return 1;
    }
    if (rust->language != c->language) {
        fprintf(stderr, "  lang %d language: %d vs %d\n", lang, (int)rust->language,
                (int)c->language);
        return 0;
    }
    if (rust->ts_factory != c->ts_factory) {
        fprintf(stderr, "  lang %d ts_factory differs\n", lang);
        return 0;
    }
#define KINDS(field) kinds_equal(rust->field, c->field, lang, #field)
    return KINDS(function_node_types) && KINDS(class_node_types) && KINDS(field_node_types) &&
           KINDS(module_node_types) && KINDS(call_node_types) && KINDS(import_node_types) &&
           KINDS(import_from_types) && KINDS(branching_node_types) &&
           KINDS(variable_node_types) && KINDS(assignment_node_types) &&
           KINDS(throw_node_types) && KINDS(decorator_node_types) &&
           KINDS(env_access_functions) && KINDS(env_access_member_patterns) &&
           str_equal(rust->throws_clause_field, c->throws_clause_field, lang,
                     "throws_clause_field") &&
           embedded_equal(rust->embedded_imports, c->embedded_imports, lang);
#undef KINDS
}

TEST(lang_specs_rows_match_c_reference) {
    int rows = 0;
    for (int lang = 0; lang < CBM_LANG_COUNT; lang++) {
        ASSERT_TRUE(spec_equal(lang));
        rows += cbm_lang_spec((CBMLanguage)lang) != NULL;
    }
    for (size_t i = 0; i < sizeof(k_probe_extra) / sizeof(k_probe_extra[0]); i++) {
        ASSERT_TRUE(spec_equal(k_probe_extra[i]));
        ASSERT_NULL(cbm_lang_spec((CBMLanguage)k_probe_extra[i]));
    }
    /* A walk that found no rows would compare nothing. */
    ASSERT_GT(rows, 150);
    PASS();
}

TEST(lang_specs_ts_language_matches_c_reference) {
    int grammars = 0;
    for (int lang = 0; lang < CBM_LANG_COUNT; lang++) {
        const TSLanguage *rust = cbm_ts_language((CBMLanguage)lang);
        ASSERT_TRUE(rust == cbm_c_reference_ts_language((CBMLanguage)lang));
        grammars += rust != NULL;
    }
    for (size_t i = 0; i < sizeof(k_probe_extra) / sizeof(k_probe_extra[0]); i++) {
        ASSERT_NULL(cbm_ts_language((CBMLanguage)k_probe_extra[i]));
        ASSERT_NULL(cbm_c_reference_ts_language((CBMLanguage)k_probe_extra[i]));
    }
    ASSERT_GT(grammars, 150);
    PASS();
}

TEST(lang_specs_conditional_blocks_match_c_reference) {
    int with_blocks = 0;
    for (int lang = -2; lang < CBM_LANG_COUNT + 2; lang++) {
        const char **rust = cbm_conditional_block_types((CBMLanguage)lang);
        ASSERT_TRUE(kinds_equal(rust, cbm_c_reference_conditional_block_types((CBMLanguage)lang),
                                lang, "conditional_block_types"));
        with_blocks += rust != NULL;
    }
    ASSERT_EQ(with_blocks, 9);
    PASS();
}

#endif /* CBM_ENABLE_TEST_SEAMS */

SUITE(lang_specs) {
#ifdef CBM_ENABLE_TEST_SEAMS
    RUN_TEST(lang_specs_rows_match_c_reference);
    RUN_TEST(lang_specs_ts_language_matches_c_reference);
    RUN_TEST(lang_specs_conditional_blocks_match_c_reference);
#endif
}
