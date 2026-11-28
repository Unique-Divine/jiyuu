/*
 * test_perl_lsp.c — Tests for the Perl Light Semantic Pass.
 *
 * Coverage mirrors tests/test_php_lsp.c, exercising the ten foundational Perl
 * resolution scenarios from .vbw-planning/phases/22-perl-lsp-semantic-resolution/
 * 22-RESEARCH.md (L331-341):
 *   1.  Method via bless-assignment       (my $o = Foo->new; $o->bar)
 *   2.  Constructor class-method type      (Foo->new returns Foo)
 *   3.  Static package call                (Foo::bar())
 *   4.  $self method dispatch              ($self = shift; $self->m)
 *   5.  @ISA inheritance
 *   6.  use parent MRO
 *   7.  use base MRO
 *   8.  Exporter import (use Mod qw(f); f())
 *   9.  require fallback (require Foo; Foo->bar)
 *   10. Unresolvable receiver emits NO spurious edge (negative test)
 *
 * The resolver populates result->resolved_calls with CBMResolvedCall edges. Per
 * the perl_lsp.c design (file header), sub QNs are `module_qn.subname` — the Perl
 * package is NOT woven into the sub QN. For these single-file fixtures the module
 * QN is `test.main` (from the cbm_extract_file "test"/"main.pl" args), so every
 * resolved sub lands at `test.main.<sub>`. The helpers below use substring
 * matching, so tests assert on the unique `main.<sub>` callee fragment. The Perl
 * package only governs method *dispatch* (which sub a receiver resolves to), not
 * the emitted QN string.
 */
#include "test_framework.h"
#include "cbm.h"
#include "../src/pipeline/lsp_resolve.h"
#include "lsp/perl_lsp.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* ── Helpers (mirror test_php_lsp.c) ───────────────────────────── */

static CBMFileResult *extract_perl(const char *source) {
    return cbm_extract_file(source, (int)strlen(source), CBM_LANG_PERL, "test", "main.pl", 0, NULL,
                            NULL);
}

static int find_resolved(const CBMFileResult *r, const char *callerSub, const char *calleeSub) {
    for (int i = 0; i < r->resolved_calls.count; i++) {
        const CBMResolvedCall *rc = &r->resolved_calls.items[i];
        if (rc->caller_qn && strstr(rc->caller_qn, callerSub) && rc->callee_qn &&
            strstr(rc->callee_qn, calleeSub))
            return i;
    }
    return -1;
}

static int require_resolved(const CBMFileResult *r, const char *callerSub, const char *calleeSub) {
    int idx = find_resolved(r, callerSub, calleeSub);
    if (idx < 0) {
        printf("  MISSING resolved call: caller~%s -> callee~%s (have %d)\n", callerSub, calleeSub,
               r->resolved_calls.count);
        for (int i = 0; i < r->resolved_calls.count; i++) {
            const CBMResolvedCall *rc = &r->resolved_calls.items[i];
            printf("    %s -> %s [%s %.2f]\n", rc->caller_qn ? rc->caller_qn : "(null)",
                   rc->callee_qn ? rc->callee_qn : "(null)", rc->strategy ? rc->strategy : "(null)",
                   rc->confidence);
        }
    }
    return idx;
}

static const CBMResolvedCall *find_resolved_with_strategy(const CBMFileResult *r,
                                                          const char *callerSub,
                                                          const char *calleeSub,
                                                          const char *strategy) {
    for (int i = 0; i < r->resolved_calls.count; i++) {
        const CBMResolvedCall *rc = &r->resolved_calls.items[i];
        if (!rc->caller_qn || !rc->callee_qn)
            continue;
        if (!strstr(rc->caller_qn, callerSub))
            continue;
        if (!strstr(rc->callee_qn, calleeSub))
            continue;
        if (strategy && (!rc->strategy || strcmp(rc->strategy, strategy) != 0))
            continue;
        return rc;
    }
    return NULL;
}

/* ── 1. Method dispatch via bless-assignment ($o = Foo->new) ───── */

TEST(perllsp_method_via_bless_assignment) {
    const char *src = "package Foo;\n"
                      "sub new { my $class = shift; return bless {}, $class; }\n"
                      "sub bar { return 1; }\n"
                      "package main;\n"
                      "sub run {\n"
                      "    my $obj = Foo->new;\n"
                      "    $obj->bar;\n"
                      "}\n";
    CBMFileResult *r = extract_perl(src);
    ASSERT(r);
    /* $obj is typed Foo via Foo->new (bless); $obj->bar dispatches to Foo::bar,
     * emitted as test.main.bar. */
    int idx = require_resolved(r, "main.run", "main.bar");
    ASSERT(idx >= 0);
    cbm_free_result(r);
    PASS();
}

/* ── 2. Constructor class-method returns the package type ──────── */

TEST(perllsp_constructor_class_method) {
    /* Foo->new must yield type Foo so the subsequent method resolves on it. */
    const char *src = "package Foo;\n"
                      "sub new { return bless {}, shift; }\n"
                      "sub greet { return 'hi'; }\n"
                      "package main;\n"
                      "sub go {\n"
                      "    my $f = Foo->new();\n"
                      "    $f->greet();\n"
                      "}\n";
    CBMFileResult *r = extract_perl(src);
    ASSERT(r);
    ASSERT(require_resolved(r, "main.go", "main.greet") >= 0);
    cbm_free_result(r);
    PASS();
}

/* ── 3. Static package-qualified call (Foo::bar()) ─────────────── */

TEST(perllsp_static_package_call) {
    const char *src = "package Foo;\n"
                      "sub bar { return 42; }\n"
                      "package main;\n"
                      "sub caller_sub {\n"
                      "    Foo::bar();\n"
                      "}\n";
    CBMFileResult *r = extract_perl(src);
    ASSERT(r);
    ASSERT(require_resolved(r, "main.caller_sub", "main.bar") >= 0);
    /* The same edge is retrievable as a full CBMResolvedCall; static calls carry
     * the perl_static_call strategy. */
    const CBMResolvedCall *rc =
        find_resolved_with_strategy(r, "main.caller_sub", "main.bar", "perl_static_call");
    ASSERT(rc != NULL);
    ASSERT(rc->strategy != NULL);
    cbm_free_result(r);
    PASS();
}

/* ── 3b. Multi-level static package call (Foo::Bar::sub()) ──────── */
/* Regression: the resolver split the qualified name on the FIRST "::", so a
 * call to Foo::Bar::sub() was mis-parsed as pkg "Foo" / sub "Bar::sub" and
 * never resolved — falling through to the bare-name fallback, which collapsed
 * distinct packages' same-named subs onto one winner. Splitting on the LAST
 * "::" keeps the full package name so perl_lookup_method resolves correctly. */
TEST(perllsp_static_multilevel_package_call) {
    const char *src = "package Acme::Widget;\n"
                      "sub render { return 1; }\n"
                      "package main;\n"
                      "sub caller_sub {\n"
                      "    Acme::Widget::render();\n"
                      "}\n";
    CBMFileResult *r = extract_perl(src);
    ASSERT(r);
    /* Before the fix this edge was absent (LSP emitted nothing for the
     * multi-level qualified call). */
    const CBMResolvedCall *rc =
        find_resolved_with_strategy(r, "main.caller_sub", "main.render", "perl_static_call");
    ASSERT(rc != NULL);
    ASSERT(rc->strategy != NULL);
    cbm_free_result(r);
    PASS();
}

/* ── 4. $self method dispatch ($self = shift) ──────────────────── */

TEST(perllsp_self_method) {
    const char *src = "package Widget;\n"
                      "sub new { return bless {}, shift; }\n"
                      "sub render { my $self = shift; $self->draw(); }\n"
                      "sub draw { return 1; }\n";
    CBMFileResult *r = extract_perl(src);
    ASSERT(r);
    ASSERT(require_resolved(r, "main.render", "main.draw") >= 0);
    cbm_free_result(r);
    PASS();
}

/* ── 5. @ISA inheritance ───────────────────────────────────────── */

TEST(perllsp_isa_inheritance) {
    /* Derived->new blesses into Derived; speak is inherited from Base via @ISA.
     * The dispatch walks Derived's embedded parent (Base) to find speak, emitted
     * as test.main.speak. */
    const char *src = "package Base;\n"
                      "sub speak { return 'base'; }\n"
                      "package Derived;\n"
                      "our @ISA = ('Base');\n"
                      "sub new { my $class = shift; return bless {}, $class; }\n"
                      "package main;\n"
                      "sub run {\n"
                      "    my $d = Derived->new;\n"
                      "    $d->speak;\n"
                      "}\n";
    CBMFileResult *r = extract_perl(src);
    ASSERT(r);
    ASSERT(require_resolved(r, "main.run", "main.speak") >= 0);
    cbm_free_result(r);
    PASS();
}

/* ── 6. use parent 'Base' MRO ──────────────────────────────────── */

TEST(perllsp_use_parent_inheritance) {
    const char *src = "package Base;\n"
                      "sub greet { return 'hi'; }\n"
                      "package Child;\n"
                      "use parent -norequire, 'Base';\n"
                      "sub new { my $class = shift; return bless {}, $class; }\n"
                      "package main;\n"
                      "sub run {\n"
                      "    my $c = Child->new;\n"
                      "    $c->greet;\n"
                      "}\n";
    CBMFileResult *r = extract_perl(src);
    ASSERT(r);
    ASSERT(require_resolved(r, "main.run", "main.greet") >= 0);
    cbm_free_result(r);
    PASS();
}

/* ── 7. use base 'Base' MRO ────────────────────────────────────── */

TEST(perllsp_use_base_inheritance) {
    const char *src = "package Base;\n"
                      "sub greet { return 'hi'; }\n"
                      "package Child;\n"
                      "use base 'Base';\n"
                      "sub new { my $class = shift; return bless {}, $class; }\n"
                      "package main;\n"
                      "sub run {\n"
                      "    my $c = Child->new;\n"
                      "    $c->greet;\n"
                      "}\n";
    CBMFileResult *r = extract_perl(src);
    ASSERT(r);
    ASSERT(require_resolved(r, "main.run", "main.greet") >= 0);
    cbm_free_result(r);
    PASS();
}

/* ── 8. Exporter import (use Module qw(func); func()) ──────────── */

TEST(perllsp_exported_function) {
    /* func() is imported from Helper; the bare call resolves to Helper::func,
     * emitted as test.main.func via the Exporter import map. */
    const char *src = "package Helper;\n"
                      "sub func { return 1; }\n"
                      "package main;\n"
                      "use Helper qw(func);\n"
                      "sub run {\n"
                      "    func();\n"
                      "}\n";
    CBMFileResult *r = extract_perl(src);
    ASSERT(r);
    ASSERT(require_resolved(r, "main.run", "main.func") >= 0);
    cbm_free_result(r);
    PASS();
}

/* ── 8b. Seeded CPAN Exporter import (use Scalar::Util qw(blessed)) ─
 *
 * Regression for the import-map "::" vs "." mismatch (QA round 1, F1):
 * perl_collect_qw_imports used to build the colon-form target
 * "Scalar::Util::blessed", but the stdlib registry keys curated CPAN subs in
 * dotted form ("Scalar.Util.blessed") and lookup is exact-match — so the
 * import never resolved. The import target must be dotted to match. */
TEST(perllsp_cpan_exported_function) {
    /* blessed is a curated CPAN export (Scalar::Util) seeded by
     * cbm_perl_stdlib_register as "Scalar.Util.blessed". The bare call must
     * resolve to that seeded registry symbol via the Exporter import map. */
    const char *src = "package main;\n"
                      "use Scalar::Util qw(blessed);\n"
                      "sub run {\n"
                      "    my $x = bless {}, 'Foo';\n"
                      "    blessed($x);\n"
                      "}\n";
    CBMFileResult *r = extract_perl(src);
    ASSERT(r);
    /* Resolves to the dotted registry QN Scalar.Util.blessed. */
    ASSERT(require_resolved(r, "main.run", "Scalar.Util.blessed") >= 0);
    cbm_free_result(r);
    PASS();
}

/* ── 9. require fallback (require Foo; Foo->bar()) ─────────────── */

TEST(perllsp_require_fallback) {
    const char *src = "package Foo;\n"
                      "sub bar { return 1; }\n"
                      "package main;\n"
                      "sub run {\n"
                      "    require Foo;\n"
                      "    Foo->bar();\n"
                      "}\n";
    CBMFileResult *r = extract_perl(src);
    ASSERT(r);
    ASSERT(require_resolved(r, "main.run", "main.bar") >= 0);
    cbm_free_result(r);
    PASS();
}

/* ── 9b. SUPER:: dispatch ($self->SUPER::method) ──────────────────
 *
 * Regression for the dead enclosing_parent_qn field (QA round 1, F4). The
 * header advertised SUPER:: support but the field was never populated/read, so
 * $self->SUPER::method() resolved to nothing. Now process_package_decl records
 * the package's first @ISA parent and perl_resolve_method_call routes a
 * SUPER:: call to that parent's method. */
TEST(perllsp_super_dispatch) {
    /* Child overrides greet and calls $self->SUPER::greet(); the SUPER call
     * must resolve to Base::greet (the parent), tagged perl_method_super. */
    const char *src = "package Base;\n"
                      "sub new { return bless {}, shift; }\n"
                      "sub greet { return 'base'; }\n"
                      "package Child;\n"
                      "our @ISA = ('Base');\n"
                      "sub greet {\n"
                      "    my $self = shift;\n"
                      "    return $self->SUPER::greet();\n"
                      "}\n";
    CBMFileResult *r = extract_perl(src);
    ASSERT(r);
    /* The SUPER:: call resolves to a greet sub via the dedicated strategy. */
    const CBMResolvedCall *rc =
        find_resolved_with_strategy(r, "main.greet", "main.greet", "perl_method_super");
    ASSERT(rc != NULL);
    cbm_free_result(r);
    PASS();
}

/* ── 9c. SUPER:: with no known parent emits NO edge (zero-edge) ─── */
TEST(perllsp_super_no_parent_no_edge) {
    /* Orphan has no @ISA parent; SUPER::greet() must resolve to nothing rather
     * than guessing an edge. */
    const char *src = "package Orphan;\n"
                      "sub greet {\n"
                      "    my $self = shift;\n"
                      "    return $self->SUPER::greet();\n"
                      "}\n";
    CBMFileResult *r = extract_perl(src);
    ASSERT(r);
    ASSERT(find_resolved_with_strategy(r, "main.greet", "greet", "perl_method_super") == NULL);
    cbm_free_result(r);
    PASS();
}

/* ── 10. Unresolvable receiver emits NO spurious edge (negative) ─ */

TEST(perllsp_unindexed_receiver_emits_block) {
    /* $thing has no inferable type (parameter from outside, never blessed/typed)
     * and Unknown::Pkg is not indexed. The resolver MUST emit zero edges for
     * these calls rather than guessing. */
    const char *src = "package main;\n"
                      "sub run {\n"
                      "    my $thing = get_external();\n"
                      "    $thing->do_work();\n"
                      "    Unknown::Pkg->mystery();\n"
                      "}\n";
    CBMFileResult *r = extract_perl(src);
    ASSERT(r);
    /* No edge for the untyped scalar receiver. */
    ASSERT(find_resolved(r, "main.run", "do_work") < 0);
    /* No edge for the unindexed package receiver. */
    ASSERT(find_resolved(r, "main.run", "mystery") < 0);
    cbm_free_result(r);
    PASS();
}

/* Occurrence identity is independent of the pending public-QN decision. This
 * deliberately invokes the same Widget method twice, so both sites resolve to
 * one target under today's flat QNs and would still share one target if that QN
 * later becomes package-qualified. Each semantic record must nevertheless be
 * tied to its own full parser occurrence. */
TEST(perllsp_repeated_target_calls_join_by_exact_site) {
    static const char source[] = "package Widget;\n"
                                 "sub render { return 1; }\n"
                                 "package main;\n"
                                 "sub occurrence_probe {\n"
                                 "    Widget->render();\n"
                                 "    Widget->render();\n"
                                 "}\n";
    static const char call_text[] = "Widget->render()";

    const char *first_site = strstr(source, call_text);
    const char *second_site = first_site ? strstr(first_site + 1, call_text) : NULL;
    ASSERT_NOT_NULL(first_site);
    ASSERT_NOT_NULL(second_site);
    const uint32_t first_start = (uint32_t)(first_site - source);
    const uint32_t first_end = first_start + (uint32_t)strlen(call_text);
    const uint32_t second_start = (uint32_t)(second_site - source);
    const uint32_t second_end = second_start + (uint32_t)strlen(call_text);

    CBMFileResult *r = extract_perl(source);
    ASSERT_NOT_NULL(r);

    const CBMCall *first_call = NULL;
    const CBMCall *second_call = NULL;
    int render_carriers = 0;
    int zero_span_carriers = 0;
    for (int i = 0; i < r->calls.count; i++) {
        const CBMCall *call = &r->calls.items[i];
        if (!call->enclosing_func_qn || !strstr(call->enclosing_func_qn, "main.occurrence_probe") ||
            !call->callee_name || !strstr(call->callee_name, "render")) {
            continue;
        }
        render_carriers++;
        if (call->site_end_byte <= call->site_start_byte)
            zero_span_carriers++;
        if (call->site_start_byte == first_start && call->site_end_byte == first_end)
            first_call = call;
        if (call->site_start_byte == second_start && call->site_end_byte == second_end)
            second_call = call;
    }

    ASSERT_EQ(render_carriers, 2);
    ASSERT_EQ(zero_span_carriers, 0);
    ASSERT_NOT_NULL(first_call);
    ASSERT_NOT_NULL(second_call);
    ASSERT_TRUE(first_call != second_call);

    const CBMResolvedCall *first_semantic = NULL;
    const CBMResolvedCall *second_semantic = NULL;
    int render_semantics = 0;
    int zero_span_semantics = 0;
    for (int i = 0; i < r->resolved_calls.count; i++) {
        const CBMResolvedCall *rc = &r->resolved_calls.items[i];
        if (rc->kind != CBM_RESOLVED_INVOCATION || rc->confidence <= 0.0f || !rc->caller_qn ||
            !strstr(rc->caller_qn, "main.occurrence_probe") || !rc->callee_qn ||
            !strstr(rc->callee_qn, "render")) {
            continue;
        }
        render_semantics++;
        if (rc->site_end_byte <= rc->site_start_byte)
            zero_span_semantics++;
        if (rc->site_start_byte == first_start && rc->site_end_byte == first_end)
            first_semantic = rc;
        if (rc->site_start_byte == second_start && rc->site_end_byte == second_end)
            second_semantic = rc;
    }

    ASSERT_EQ(render_semantics, 2);
    ASSERT_EQ(zero_span_semantics, 0);
    ASSERT_NOT_NULL(first_semantic);
    ASSERT_NOT_NULL(second_semantic);
    ASSERT_TRUE(first_semantic != second_semantic);
    ASSERT_STR_EQ(first_semantic->callee_qn, second_semantic->callee_qn);

    const CBMResolvedCall *first_joined =
        cbm_pipeline_find_lsp_resolution(&r->resolved_calls, first_call, false);
    const CBMResolvedCall *second_joined =
        cbm_pipeline_find_lsp_resolution(&r->resolved_calls, second_call, false);
    ASSERT_TRUE(first_joined == first_semantic);
    ASSERT_TRUE(second_joined == second_semantic);

    cbm_free_result(r);
    PASS();
}

/* Keep the other ordinary resolver branches under the same occurrence
 * contract. The first regression above is a static method-call expression;
 * this helper also exercises function-call expressions and typed/inherited
 * method dispatch without coupling the tests to the pending package-QN
 * decision. */
static int assert_perl_repeated_exact_join(const char *source, const char *caller_fragment,
                                           const char *callee_fragment, const char *call_text) {
    const char *first_site = strstr(source, call_text);
    const char *second_site = first_site ? strstr(first_site + 1, call_text) : NULL;
    ASSERT_NOT_NULL(first_site);
    ASSERT_NOT_NULL(second_site);
    const uint32_t starts[2] = {(uint32_t)(first_site - source), (uint32_t)(second_site - source)};
    const uint32_t ends[2] = {starts[0] + (uint32_t)strlen(call_text),
                              starts[1] + (uint32_t)strlen(call_text)};

    CBMFileResult *r = extract_perl(source);
    ASSERT_NOT_NULL(r);

    const CBMCall *calls[2] = {NULL, NULL};
    int carrier_count = 0;
    for (int i = 0; i < r->calls.count; i++) {
        const CBMCall *call = &r->calls.items[i];
        if (!call->enclosing_func_qn || !strstr(call->enclosing_func_qn, caller_fragment) ||
            !call->callee_name || !strstr(call->callee_name, callee_fragment)) {
            continue;
        }
        carrier_count++;
        for (int occurrence = 0; occurrence < 2; occurrence++) {
            if (call->site_start_byte == starts[occurrence] &&
                call->site_end_byte == ends[occurrence]) {
                calls[occurrence] = call;
            }
        }
    }
    ASSERT_EQ(carrier_count, 2);
    ASSERT_NOT_NULL(calls[0]);
    ASSERT_NOT_NULL(calls[1]);

    const CBMResolvedCall *semantics[2] = {NULL, NULL};
    int semantic_count = 0;
    int zero_span_semantics = 0;
    for (int i = 0; i < r->resolved_calls.count; i++) {
        const CBMResolvedCall *resolved = &r->resolved_calls.items[i];
        if (resolved->kind != CBM_RESOLVED_INVOCATION || resolved->confidence <= 0.0f ||
            !resolved->caller_qn || !strstr(resolved->caller_qn, caller_fragment) ||
            !resolved->callee_qn || !strstr(resolved->callee_qn, callee_fragment)) {
            continue;
        }
        semantic_count++;
        if (resolved->site_end_byte <= resolved->site_start_byte) {
            zero_span_semantics++;
        }
        for (int occurrence = 0; occurrence < 2; occurrence++) {
            if (resolved->site_start_byte == starts[occurrence] &&
                resolved->site_end_byte == ends[occurrence]) {
                semantics[occurrence] = resolved;
            }
        }
    }
    ASSERT_EQ(semantic_count, 2);
    ASSERT_EQ(zero_span_semantics, 0);
    ASSERT_NOT_NULL(semantics[0]);
    ASSERT_NOT_NULL(semantics[1]);
    ASSERT_STR_EQ(semantics[0]->callee_qn, semantics[1]->callee_qn);
    ASSERT_TRUE(cbm_pipeline_find_lsp_resolution(&r->resolved_calls, calls[0], false) ==
                semantics[0]);
    ASSERT_TRUE(cbm_pipeline_find_lsp_resolution(&r->resolved_calls, calls[1], false) ==
                semantics[1]);

    cbm_free_result(r);
    return 0;
}

TEST(perllsp_repeated_static_function_calls_join_by_exact_site) {
    static const char source[] = "package Helper;\n"
                                 "sub work { return 1; }\n"
                                 "package main;\n"
                                 "sub function_occurrence_probe {\n"
                                 "    Helper::work();\n"
                                 "    Helper::work();\n"
                                 "}\n";
    return assert_perl_repeated_exact_join(source, "main.function_occurrence_probe", "work",
                                           "Helper::work()");
}

TEST(perllsp_repeated_inherited_method_calls_join_by_exact_site) {
    static const char source[] = "package Base;\n"
                                 "sub greet { return 1; }\n"
                                 "package Child;\n"
                                 "our @ISA = ('Base');\n"
                                 "sub new { my $class = shift; return bless {}, $class; }\n"
                                 "package main;\n"
                                 "sub inherited_occurrence_probe {\n"
                                 "    my $child = Child->new();\n"
                                 "    $child->greet();\n"
                                 "    $child->greet();\n"
                                 "}\n";
    return assert_perl_repeated_exact_join(source, "main.inherited_occurrence_probe", "greet",
                                           "$child->greet()");
}

/* ── Package lookup (Rust package index) ───────────────────────── */

static bool perl_str_eq(const char *a, const char *b) {
    return a == b || (a && b && strcmp(a, b) == 0);
}

static void perl_print_resolved(const char *side, const CBMFileResult *r) {
    for (int i = 0; i < r->resolved_calls.count; i++) {
        const CBMResolvedCall *rc = &r->resolved_calls.items[i];
        printf("    %s %s -> %s [%s %.2f] %u:%u\n", side, rc->caller_qn ? rc->caller_qn : "(null)",
               rc->callee_qn ? rc->callee_qn : "(null)", rc->strategy ? rc->strategy : "(null)",
               rc->confidence, rc->site_start_byte, rc->site_end_byte);
    }
}

/* Drop the fixture module prefix so expectations stay short. */
static const char *perl_short_qn(const char *qn) {
    static const char prefix[] = "test.main.";
    if (!qn)
        return "(null)";
    return strncmp(qn, prefix, sizeof(prefix) - 1) == 0 ? qn + sizeof(prefix) - 1 : qn;
}

/* Require the resolved calls of `source`, one "caller -> callee strategy
 * confidence start:end" line each, in order. These expectations were the
 * output of the C registry lookup, which matched the Rust package index on
 * every case before the C lookup was deleted. */
static int perl_assert_resolved_lines(const char *label, const char *source, const char *expected) {
    CBMFileResult *r = extract_perl(source);
    ASSERT_NOT_NULL(r);
    char actual[4096];
    size_t used = 0;
    actual[0] = '\0';
    for (int i = 0; i < r->resolved_calls.count && used < sizeof(actual); i++) {
        const CBMResolvedCall *rc = &r->resolved_calls.items[i];
        ASSERT_EQ(rc->kind, CBM_RESOLVED_INVOCATION);
        ASSERT_TRUE(rc->reason == NULL && rc->callee_sig == NULL);
        used += (size_t)snprintf(actual + used, sizeof(actual) - used, "%s -> %s %s %.2f %u:%u\n",
                                 perl_short_qn(rc->caller_qn), perl_short_qn(rc->callee_qn),
                                 rc->strategy ? rc->strategy : "(null)", rc->confidence,
                                 rc->site_start_byte, rc->site_end_byte);
    }
    cbm_free_result(r);
    ASSERT_TRUE(used < sizeof(actual));
    if (strcmp(actual, expected) != 0) {
        printf("  Perl resolved calls differ for %s; got:\n", label);
        for (const char *line = actual; *line;) {
            const char *end = strchr(line, '\n');
            printf("         \"%.*s\\n\"\n", (int)(end - line), line);
            line = end + 1;
        }
    }
    ASSERT_STR_EQ(actual, expected);
    return 0;
}

/* Each source exercises one branch of the package lookup: own subs, @ISA
 * order, SUPER, cycles and block or reopened packages. */
TEST(perllsp_package_lookup_cases) {
    static const struct {
        const char *label;
        const char *source;
        const char *expected;
    } cases[] = {
        {"empty file", "", ""},
        {"main only", "package main;\n", ""},
        {"subs before any package",
         "sub helper { return 1; }\n"
         "sub run { main->helper(); __PACKAGE__->helper(); main->missing(); }\n",
         "run -> helper perl_method_inherited 0.95 35:49\n"},
        {"own sub and constructor",
         "package Acme::Widget;\n"
         "sub new { my $class = shift; return bless {}, $class; }\n"
         "sub render { return 1; }\n"
         "package main;\n"
         "sub run { my $w = Acme::Widget->new; $w->render; Widget->render; }\n",
         "run -> new perl_method_inherited 0.95 135:152\n"
         "run -> render perl_method_inherited 0.95 154:164\n"},
        {"parent without subs",
         "package Base;\n"
         "sub speak { return 1; }\n"
         "package Intermediate;\n"
         "our @ISA = ('Base');\n"
         "package Derived;\n"
         "use parent -norequire, 'Intermediate';\n"
         "sub new { return bless {}, shift; }\n"
         "package main;\n"
         "sub run { my $d = Derived->new; $d->speak; Derived->speak; Derived->absent; }\n",
         "run -> new perl_method_inherited 0.95 205:217\n"
         "run -> speak perl_method_inherited 0.95 219:228\n"
         "run -> speak perl_method_inherited 0.95 230:244\n"},
        {"several parents and SUPER",
         "package First;\n"
         "sub greet { return 1; }\n"
         "package Last;\n"
         "sub greet { return 2; }\n"
         "sub only_last { return 3; }\n"
         "package Child;\n"
         "use base qw(First Last);\n"
         "sub new { return bless {}, shift; }\n"
         "sub greet { my $self = shift; return $self->SUPER::greet(); }\n"
         "sub last { my $self = shift; return $self->SUPER::only_last(); }\n"
         "sub none { my $self = shift; return $self->SUPER::missing(); }\n"
         "package main;\n"
         "sub run { my $c = Child->new; $c->greet; $c->only_last; Child->greet; }\n",
         "run -> new perl_method_inherited 0.95 403:413\n"
         "run -> greet perl_method_inherited 0.95 415:424\n"
         "run -> greet perl_method_inherited 0.95 441:453\n"},
        {"SUPER without a parent",
         "package Orphan;\n"
         "sub greet { my $self = shift; return $self->SUPER::greet(); }\n"
         "sub empty { my $self = shift; return $self->SUPER::(); }\n",
         ""},
        {"block packages",
         "package Base { sub greet { return 1; } }\n"
         "package Child {\n"
         "    our @ISA = ('Base');\n"
         "    sub new { return bless {}, shift; }\n"
         "    sub greet { my $self = shift; return $self->SUPER::greet(); }\n"
         "}\n"
         "sub after_block { my $self = shift; return $self->SUPER::greet(); }\n"
         "package main;\n"
         "sub run { my $c = Child->new; $c->greet; }\n",
         ""},
        {"reopened package",
         "package Base;\n"
         "sub greet { return 1; }\n"
         "package Child;\n"
         "sub new { return bless {}, shift; }\n"
         "package Other;\n"
         "sub other { return 1; }\n"
         "package Child;\n"
         "our @ISA = ('Base');\n"
         "sub again { my $self = shift; return $self->SUPER::greet(); }\n"
         "package main;\n"
         "sub run { Child->greet; Child->other; }\n",
         "again -> greet perl_method_super 0.95 201:222\n"
         "run -> greet perl_method_inherited 0.95 250:262\n"},
        {"qualified @ISA and duplicate subs",
         "package Base;\n"
         "sub greet { return 1; }\n"
         "sub greet { return 2; }\n"
         "package Child;\n"
         "@Child::ISA = ('Base', 'Base');\n"
         "package main;\n"
         "sub run { Child->greet; }\n",
         "run -> greet perl_method_inherited 0.95 133:145\n"},
        {"inheritance cycle",
         "package A;\n"
         "our @ISA = ('B');\n"
         "sub from_a { return 1; }\n"
         "package B;\n"
         "our @ISA = ('A');\n"
         "package main;\n"
         "sub run { B->from_a; A->missing; B->missing; }\n",
         "run -> from_a perl_method_inherited 0.95 107:116\n"},
        {"exported and static calls",
         "package Helper;\n"
         "sub func { return 1; }\n"
         "package main;\n"
         "use Helper qw(func);\n"
         "use Scalar::Util qw(blessed);\n"
         "sub run { func(); Helper::func(); blessed(1); Helper->func; }\n",
         "run -> func perl_function_local 0.95 114:120\n"
         "run -> func perl_static_call 0.95 122:136\n"
         "run -> Scalar.Util.blessed perl_imported_function 0.95 138:148\n"
         "run -> func perl_method_inherited 0.95 150:162\n"},
    };
    int failures = 0;
    for (size_t i = 0; i < sizeof(cases) / sizeof(cases[0]); i++)
        failures += perl_assert_resolved_lines(cases[i].label, cases[i].source, cases[i].expected);
    ASSERT_EQ(failures, 0);
    PASS();
}

/* An @ISA chain one package longer than the 32-package visit cap, and a
 * package with more parents than the 32-entry frontier holds. */
TEST(perllsp_package_lookup_at_caps) {
    enum { LINKS = 34, PARENTS = 40 };
    char chain[8192];
    size_t used = 0;
    for (int i = 0; i < LINKS; i++) {
        used += (size_t)snprintf(chain + used, sizeof(chain) - used,
                                 "package P%d;\nour @ISA = ('P%d');\n", i, i + 1);
    }
    used += (size_t)snprintf(chain + used, sizeof(chain) - used,
                             "package P%d;\nsub deep { return 1; }\n"
                             "package main;\n"
                             "sub run { P0->deep; P1->deep; P2->deep; P3->deep; }\n",
                             LINKS);
    ASSERT_TRUE(used < sizeof(chain));
    int failures = perl_assert_resolved_lines("chain past the visit cap", chain,
                                              "run -> deep perl_method_inherited 0.95 1193:1201\n");

    char fan[8192];
    used = (size_t)snprintf(fan, sizeof(fan), "package Wide;\nour @ISA = (");
    for (int i = 0; i < PARENTS; i++)
        used += (size_t)snprintf(fan + used, sizeof(fan) - used, "%s'Q%d'", i ? ", " : "", i);
    used += (size_t)snprintf(fan + used, sizeof(fan) - used, ");\n");
    for (int i = 0; i < PARENTS; i++)
        used += (size_t)snprintf(fan + used, sizeof(fan) - used,
                                 "package Q%d;\nsub q%d { return 1; }\n", i, i);
    used += (size_t)snprintf(fan + used, sizeof(fan) - used,
                             "package main;\nsub run { Wide->q0; Wide->q8; Wide->q31; "
                             "Wide->q32; Wide->q39; }\n");
    ASSERT_TRUE(used < sizeof(fan));
    failures += perl_assert_resolved_lines("parents past the frontier cap", fan,
                                           "run -> q8 perl_method_inherited 0.95 1711:1719\n"
                                           "run -> q31 perl_method_inherited 0.95 1721:1730\n");
    ASSERT_EQ(failures, 0);
    PASS();
}

/* SUPER in code before the first `package` statement has no enclosing
 * package, and the implicit main package has no @ISA, so it resolves to
 * nothing. The old C lookup emitted a stale perl_method_super edge here
 * (early -> greet): its resolve walk kept the SUPER parent that the second
 * pre-pass left for the reopened `package Last;`. */
TEST(perllsp_package_lookup_super_before_first_package) {
    static const char source[] = "sub early { my $self = shift; return $self->SUPER::greet(); }\n"
                                 "package Base;\n"
                                 "sub greet { return 1; }\n"
                                 "package Last;\n"
                                 "our @ISA = ('Base');\n"
                                 "package Other;\n"
                                 "package Last;\n";
    return perl_assert_resolved_lines("SUPER before the first package", source, "");
}

/* Read a resolver fixture relative to the mori/ directory the runner uses. */
static char *perl_read_fixture(const char *path) {
    FILE *f = fopen(path, "rb");
    if (!f)
        return NULL;
    char *buf = NULL;
    long len = -1;
    if (fseek(f, 0, SEEK_END) == 0)
        len = ftell(f);
    if (len >= 0 && fseek(f, 0, SEEK_SET) == 0)
        buf = malloc((size_t)len + 1);
    if (buf && fread(buf, 1, (size_t)len, f) == (size_t)len) {
        buf[len] = '\0';
    } else {
        free(buf);
        buf = NULL;
    }
    fclose(f);
    return buf;
}

/* resolver_acceptance.rs checks the Perl rows of expected.tsv in the graph,
 * which merges repeated calls into one CALLS edge. Check each listed span
 * here: every occurrence needs its own resolved call. */
static int perl_assert_fixture_occurrences(void) {
    char *table = perl_read_fixture("tests/fixtures/resolver/expected.tsv");
    ASSERT_NOT_NULL(table);
    int rows = 0;
    char *save_row = NULL;
    for (char *row = strtok_r(table, "\n", &save_row); row; row = strtok_r(NULL, "\n", &save_row)) {
        char *col[7] = {0};
        int n = 0;
        for (char *p = row; n < 7;) {
            col[n++] = p;
            char *tab = strchr(p, '\t');
            if (!tab)
                break;
            *tab = '\0';
            p = tab + 1;
        }
        size_t path_len = strlen(col[0]);
        if (row[0] == '#' || path_len < 3 || strcmp(col[0] + path_len - 3, ".pl") != 0)
            continue;
        ASSERT_EQ(n, 7);
        rows++;

        char path[256];
        snprintf(path, sizeof(path), "tests/fixtures/resolver/%s", col[0]);
        char *source = perl_read_fixture(path);
        ASSERT_NOT_NULL(source);
        char module[128];
        snprintf(module, sizeof(module), "test.%.*s.main", (int)strcspn(col[0], "/"), col[0]);
        char caller[256];
        char callee[256];
        snprintf(caller, sizeof(caller), "%s.%s", module, col[1]);
        snprintf(callee, sizeof(callee), "%s.%s", module, col[2]);

        CBMFileResult *r = cbm_extract_file(source, (int)strlen(source), CBM_LANG_PERL, "test",
                                            col[0], 0, NULL, NULL);
        ASSERT_NOT_NULL(r);
        int spans = 0;
        int matched = 0;
        char *save_span = NULL;
        for (char *span = strtok_r(col[4], ",", &save_span); span;
             span = strtok_r(NULL, ",", &save_span)) {
            unsigned start = 0;
            unsigned end = 0;
            ASSERT_EQ(sscanf(span, "%u:%u", &start, &end), 2);
            spans++;
            for (int i = 0; i < r->resolved_calls.count; i++) {
                const CBMResolvedCall *rc = &r->resolved_calls.items[i];
                if (rc->kind == CBM_RESOLVED_INVOCATION && rc->site_start_byte == start &&
                    rc->site_end_byte == end && perl_str_eq(rc->caller_qn, caller) &&
                    perl_str_eq(rc->callee_qn, callee)) {
                    matched++;
                    break;
                }
            }
        }
        if (matched != spans) {
            printf("  %s: %d of %d spans resolve %s -> %s\n", col[0], matched, spans, caller,
                   callee);
            perl_print_resolved("got ", r);
        }
        cbm_free_result(r);
        free(source);
        ASSERT_GT(spans, 1);
        ASSERT_EQ(matched, spans);
    }
    free(table);
    ASSERT_GT(rows, 0);
    return 0;
}

TEST(perllsp_fixture_occurrences_resolve_by_site) {
    return perl_assert_fixture_occurrences();
}

/* ── Suite registration ────────────────────────────────────────── */

SUITE(perl_lsp) {
    RUN_TEST(perllsp_method_via_bless_assignment);
    RUN_TEST(perllsp_constructor_class_method);
    RUN_TEST(perllsp_static_package_call);
    RUN_TEST(perllsp_static_multilevel_package_call);
    RUN_TEST(perllsp_self_method);
    RUN_TEST(perllsp_isa_inheritance);
    RUN_TEST(perllsp_use_parent_inheritance);
    RUN_TEST(perllsp_use_base_inheritance);
    RUN_TEST(perllsp_exported_function);
    RUN_TEST(perllsp_cpan_exported_function);
    RUN_TEST(perllsp_require_fallback);
    RUN_TEST(perllsp_super_dispatch);
    RUN_TEST(perllsp_super_no_parent_no_edge);
    RUN_TEST(perllsp_unindexed_receiver_emits_block);
    RUN_TEST(perllsp_repeated_target_calls_join_by_exact_site);
    RUN_TEST(perllsp_repeated_static_function_calls_join_by_exact_site);
    RUN_TEST(perllsp_repeated_inherited_method_calls_join_by_exact_site);
    RUN_TEST(perllsp_package_lookup_cases);
    RUN_TEST(perllsp_package_lookup_at_caps);
    RUN_TEST(perllsp_package_lookup_super_before_first_package);
    RUN_TEST(perllsp_fixture_occurrences_resolve_by_site);
}
