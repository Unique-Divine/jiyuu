use std::ffi::c_long;

use super::{MAX_MEMBERS, Name, Param, Shape, charge_budget, parse};

fn render_name(label: &str, name: Name<'_>) -> String {
    let bytes = match name {
        Name::Bare(name) => name.to_vec(),
        Name::Qualified(module, name) => [module, b".", name].concat(),
    };
    atom(label, &bytes)
}

fn atom(label: &str, bytes: &[u8]) -> String {
    let hex = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    format!("{label}({hex})")
}

fn render(shape: Shape<'_>, children: &[String]) -> String {
    match shape {
        Shape::Unknown => "?".into(),
        Shape::Named(name) => render_name("named", name),
        Shape::Builtin(name) => atom("builtin", name),
        Shape::This => "this".into(),
        Shape::Infer(name) => atom("infer", name),
        Shape::Typeof(expr) => atom("typeof", expr),
        Shape::Keyof => format!("keyof({})", children[0]),
        Shape::Conditional => format!("conditional({})", children.join(",")),
        Shape::Function(params) => {
            let params = params
                .into_iter()
                .zip(&children[1..])
                .map(|(name, ty)| {
                    let name = match name {
                        Param::Explicit(name) => atom("param", name),
                        Param::Positional(index) => {
                            atom("param", format!("_{index}").as_bytes())
                        }
                    };
                    format!("{name}={ty}")
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("fn({params})->{}", children[0])
        }
        Shape::Tuple => format!("tuple({})", children.join(",")),
        Shape::Union => format!("union({})", children.join(",")),
        Shape::Intersection => format!("intersection({})", children.join(",")),
        Shape::Template(name) => {
            format!("{}<{}>", render_name("template", name), children.join(","))
        }
    }
}

fn ty(text: &[u8]) -> String {
    parse(text, Some(b"pkg.api"), |_| true, render)
}

#[test]
fn names_use_the_registry_qualification_rules() {
    for (text, expected) in [
        (&b"\r\n: Client ; "[..], atom("named", b"pkg.api.Client")),
        (b"exports.Client", atom("named", b"exports.Client")),
        (b"Document", atom("named", b"Document")),
        (b"Pick", atom("named", b"pkg.api.Pick")),
        (b"Child\xff", atom("named", b"pkg.api.Child\xff")),
        (b"unknown", atom("builtin", b"unknown")),
        (b"{ child: Client }", "?".into()),
        (b"(Client)", "?".into()),
        (b"this", "this".into()),
    ] {
        assert_eq!(ty(text), expected, "{text:?}");
    }
    assert_eq!(
        parse(b"Client", None, |_| true, render),
        atom("named", b"Client")
    );
    assert_eq!(
        parse(b"Client", Some(b""), |_| true, render),
        atom("named", b".Client")
    );
}

#[test]
fn parses_conditional_operators_and_keeps_raw_query_suffix() {
    assert_eq!(
        ty(b"T extends Box<infer R> ? keyof R : typeof value;"),
        format!(
            "conditional({},{},keyof({}),{})",
            atom("named", b"pkg.api.T"),
            format_args!(
                "{}<{}>",
                atom("template", b"pkg.api.Box"),
                atom("infer", b"R")
            ),
            atom("named", b"pkg.api.R"),
            atom("typeof", b"value")
        )
    );
    assert_eq!(ty(b": typeof exported;  "), atom("typeof", b"exported;  "));
    assert_eq!(ty(b"infer R;"), atom("infer", b"R;"));
    assert_eq!(
        ty(b"Box<T extends U ? X : Y>"),
        format!(
            "{}<{}>",
            atom("template", b"pkg.api.Box"),
            ty(b"T extends U ? X : Y")
        )
    );
}

#[test]
fn keeps_member_order_and_reference_delimiter_rules() {
    let a = atom("named", b"pkg.api.A");
    let b = atom("named", b"pkg.api.B");
    let c = atom("named", b"pkg.api.C");
    assert_eq!(ty(b"A & B | C"), format!("union({a},{b},{c})"));
    assert_eq!(ty(b"A & B"), format!("intersection({a},{b})"));
    assert_eq!(ty(b"A|B"), atom("named", b"pkg.api.A|B"));
    assert_eq!(ty(b"[]"), "tuple(?)");
    assert_eq!(ty(b"["), atom("named", b"["));
    assert_eq!(ty(b"]"), atom("named", b"]"));
    assert_eq!(ty(b"Map<>"), format!("{}<?>", atom("template", b"Map")));
    assert_eq!(
        ty(b"Provider<A, B>[]"),
        format!(
            "{}<{}<{a},{b}>>",
            atom("template", b"Array"),
            atom("template", b"pkg.api.Provider")
        )
    );
    let items = (0..20).map(|i| format!("T{i}")).collect::<Vec<_>>();
    for (input, label) in [
        (format!("[{}]", items.join(",")), "tuple"),
        (items.join(" | "), "union"),
        (items.join(" & "), "intersection"),
    ] {
        assert_eq!(ty(input.as_bytes()).matches("named(").count(), MAX_MEMBERS);
        assert!(ty(input.as_bytes()).starts_with(label));
    }
    assert_eq!(
        ty(format!("Box<{}>", items.join(",")).as_bytes())
            .matches("named(")
            .count(),
        MAX_MEMBERS
    );
    assert_eq!(
        ty(format!("({}) => void", items.join(",")).as_bytes())
            .matches("param(")
            .count(),
        MAX_MEMBERS
    );
}

#[test]
fn preserves_callable_param_slots_and_return_order() {
    let text =
        b"(handler?: Promise<Session>, number, , flag\t?: boolean) => Result[];";
    let mut charged = Vec::new();
    let actual = parse(
        text,
        Some(b"pkg.api"),
        |len| {
            charged.push(len);
            true
        },
        render,
    );
    assert_eq!(
        actual,
        format!(
            "fn({}={},{}={},{}=?,{}={})->{}",
            atom("param", b"handler"),
            ty(b"Promise<Session>"),
            atom("param", b"_1"),
            ty(b"number"),
            atom("param", b"_2"),
            atom("param", b"flag\t"),
            ty(b"boolean"),
            ty(b"Result[]")
        )
    );
    assert_eq!(charged[1], b"Result[]".len());
    assert_eq!(charged[2], b"Result".len());
    assert_eq!(charged[3], b"Promise<Session>".len());
}

#[test]
fn shares_the_reference_budget_and_its_overrun_state() {
    let mut budget = 1;
    let actual = parse(
        b"A | B",
        Some(b"pkg.api"),
        |len| charge_budget(&mut budget, len),
        render,
    );
    assert_eq!(actual, format!("union(?,{})", atom("named", b"pkg.api.B")));
    assert_eq!(budget, -1);
    assert_eq!(
        parse(
            b"number",
            None,
            |len| charge_budget(&mut budget, len),
            render
        ),
        atom("builtin", b"number")
    );
    assert_eq!(budget, -1);
}

#[test]
fn nested_arrays_do_not_recurse_on_the_native_stack() {
    let mut text = b"number".to_vec();
    let depth = 2_048;
    for _ in 0..depth {
        text.extend_from_slice(b"[]");
    }
    let count = parse(
        &text,
        None,
        |_| true,
        |shape, children: &[usize]| match shape {
            Shape::Builtin(b"number") => 0,
            Shape::Template(Name::Bare(b"Array")) => children[0] + 1,
            other => panic!("{other:?}"),
        },
    );
    assert_eq!(count, depth);
}

// Inputs from the C/Rust comparison. The transcript records the rendered type
// and remaining budget for each case. The linked TS LSP suite covers null
// input, the warning flag, and allocation failure through the resolver wrapper.
const TEXTS: &[&[u8]] = &[
    b"",
    b" ",
    b" \t\r\n",
    b":",
    b": ",
    b";",
    b",;, ",
    b"\r\n: Client ; ",
    b"Client",
    b"  Client,",
    b":Client",
    b"::Client",
    b"exports.Client",
    b"a.b.Client",
    b"Document",
    b"JSX",
    b"Pick",
    b"Promise",
    b"lower",
    b"_Under",
    b"Child\xff",
    b"\xc3\x89cole",
    b"string",
    b"number",
    b"boolean",
    b"bigint",
    b"any",
    b"unknown",
    b"void",
    b"never",
    b"null",
    b"undefined",
    b"object",
    b"symbol",
    b"String",
    b"strings",
    b"this",
    b"this;",
    b"  this  ",
    b"This",
    b"{ child: Client }",
    b"{",
    b"}",
    b"(",
    b")",
    b"(a",
    b"(Client)",
    b"(a) b",
    b"()",
    b"() =>",
    b"() => ",
    b"()=>void",
    b"() => void",
    b"() =>void;",
    b"(a: A) => R",
    b"(a: A, b?: B) => R",
    b"(handler?: Promise<Session>, number, , flag\t?: boolean) => Result[];",
    b"(cb: (x: number) => void, y: string) => void",
    b"(cb: (x: number) => Foo) => Bar",
    b"(m: Map<K, V>, t: [A, B]) => Promise<R>",
    b"( a : A ) =>  R ;",
    b"(: A) => R",
    b"(a:) => R",
    b"(...args: Arg[]) => Out",
    b"(a: A) => (b: B) => C",
    b"((a: A) => B)",
    b"(a: A) =>\tR",
    b"(a: A)\t=> R",
    b"(a: A)\n=> R",
    b"(a, b, c, d, e, f, g, h, i, j, k, l, m, n, o, p, q, r) => Z",
    b"(a,b) => (c) => d",
    b"T extends U ? X : Y",
    b"T extends U ? X : Y;",
    b"T extends Box<infer R> ? keyof R : typeof value;",
    b"T extends (infer U)[] ? U : never",
    b"T extends [infer H, ...infer Rest] ? H : never",
    b"T extends U ? X",
    b"T extends U : X ? Y",
    b"T extends U ? X : Y extends Z ? A : B",
    b"Box<T extends U ? X : Y>",
    b" extends U ? X : Y",
    b"T  extends U ? X : Y",
    b"T extends U ?X : Y",
    b"A | B extends C ? D : E",
    b"(T extends U ? X : Y)",
    b"infer R",
    b"infer R;",
    b"infer  R extra",
    b"infer \tR",
    b"infer ",
    b"infer",
    b"inferR",
    b"keyof Widget",
    b"keyof  Widget ;",
    b"keyof typeof obj",
    b"keyof ",
    b"keyof",
    b"typeof exported",
    b": typeof exported;  ",
    b"typeof \tvalue.inner",
    b"typeof ",
    b"typeof",
    b"typeofx",
    b"[]",
    b"[",
    b"]",
    b"[A]",
    b"[A, B]",
    b"[A, , B]",
    b"[A, B,]",
    b"[Map<K, V>, [C, D]]",
    b"[A, B] | C",
    b"[A][]",
    b"[T0,T1,T2,T3,T4,T5,T6,T7,T8,T9,T10,T11,T12,T13,T14,T15,T16,T17,T18,T19]",
    b"A | B",
    b"A & B",
    b"A & B | C",
    b"A | B & C",
    b"A|B",
    b"A |B",
    b"A| B",
    b"A\t| B",
    b"| A",
    b" | A",
    b"A | ",
    b"A |",
    b"A | | B",
    b"Box<A | B> | C",
    b"(A | B)",
    b"A | B[]",
    b"string | null",
    b"Promise<A> & Map<K, V>",
    b"T0 | T1 | T2 | T3 | T4 | T5 | T6 | T7 | T8 | T9 | T10 | T11 | T12 | T13 | T14 | T15 | T16",
    b"T0 & T1 & T2 & T3 & T4 & T5 & T6 & T7 & T8 & T9 & T10 & T11 & T12 & T13 & T14 & T15",
    b"T0 | T1 | T2 | T3 | T4 | T5 | T6 | T7 | T8 | T9 | T10 | T11 | T12 | T13 | T14",
    b"T0 | T1 | T2 | T3 | T4 | T5 | T6 | T7 | T8 | T9 | T10 | T11 | T12 | T13 | T14 | T15",
    b"T[]",
    b"T[][]",
    b"string[]",
    b"Provider<A, B>[]",
    b"[]]",
    b"[][]",
    b"Map<>",
    b"Map<K, V>",
    b"Foo<A, B<C>>",
    b"Foo<A,B<C,D>,E>",
    b"Foo<(a: A) => B>",
    b"Foo<[A, B]>",
    b"Foo< A , B >",
    b"Foo<A>>",
    b"Foo<<A>",
    b"<T>",
    b"<>",
    b"x<T>",
    b"a.b.Foo<T>",
    b"Partial<Widget>",
    b"Readonly<Widget>",
    b"Array<Widget>",
    b"NodeList<T>",
    b"Math<T>",
    b"Record<string, Widget>",
    b"Box<T0,T1,T2,T3,T4,T5,T6,T7,T8,T9,T10,T11,T12,T13,T14,T15,T16,T17>",
    b"Box<,>",
    b"Foo<A>.Bar",
    b"Foo<A>[]",
    b"Foo[]<A>",
    b"\"literal\"",
    b"42",
    b"`tmpl${T}`",
    b"A extends B",
    b"readonly Widget[]",
    b"new () => Widget",
    b"asserts x is Widget",
    b"x is Widget",
    b"unique symbol",
    b"Widget[\"key\"]",
    b"{ [K in keyof T]: T[K] }",
];

const MODULES: &[Option<&[u8]>] =
    &[Some(b"pkg.api"), None, Some(b""), Some(b"a.b.c")];

const BUDGET_TEXTS: &[&[u8]] = &[
    b"A | B",
    b"Client",
    b"(handler?: Promise<Session>, number, , flag\t?: boolean) => Result[];",
    b"T extends Box<infer R> ? keyof R : typeof value;",
    b"[Map<K, V>, [C, D]] | Foo<A, B<C>>[]",
    b"T0 | T1 | T2 | T3 | T4 | T5 | T6 | T7 | T8 | T9 | T10 | T11 | T12 | T13 | T14 | T15 | T16",
    b"a_very_long_identifier_name_that_spans_more_than_sixteen_bytes_twice<A, B>",
    b"",
    b"   ",
];

fn escape(text: &[u8]) -> String {
    format!("\"{}\"", text.escape_ascii())
}

fn transcript() -> String {
    let mut lines = Vec::new();
    let mut line = |text: &[u8], module: Option<&[u8]>, budget: c_long| {
        let mut remaining = budget;
        let ty = parse(
            text,
            module,
            |len| charge_budget(&mut remaining, len),
            render,
        );
        let module = module.map_or("(null)".into(), escape);
        lines.push(format!(
            "{}\t{module}\t{budget}\t{ty}\t{remaining}",
            escape(text)
        ));
    };
    for &module in MODULES {
        for text in TEXTS {
            line(text, module, -1);
        }
    }
    for text in BUDGET_TEXTS {
        for budget in -1..=40 {
            line(text, Some(b"pkg.api"), budget);
        }
    }
    for depth in (0..=8).map(|shift| 1usize << shift) {
        let generic =
            format!("{}Leaf{}", "Box<".repeat(depth), ">".repeat(depth));
        line(generic.as_bytes(), Some(b"pkg.api"), -1);
        line(generic.as_bytes(), Some(b"pkg.api"), 200);
        let array = format!("number{}", "[]".repeat(depth));
        line(array.as_bytes(), None, -1);
        let returns: String = (0..depth).map(|i| format!(") => R{i}")).collect();
        let function = format!("{}X{returns}", "(a: ".repeat(depth));
        line(function.as_bytes(), Some(b"pkg.api"), -1);
    }
    lines.join("\n") + "\n"
}

#[test]
fn matches_the_retired_c_reference_transcript() {
    let expected = include_str!("ts_type_text_cases.tsv");
    let actual = transcript();
    for (i, (want, got)) in expected.lines().zip(actual.lines()).enumerate() {
        assert_eq!(got, want, "transcript line {}", i + 1);
    }
    assert_eq!(actual.lines().count(), expected.lines().count());
}
