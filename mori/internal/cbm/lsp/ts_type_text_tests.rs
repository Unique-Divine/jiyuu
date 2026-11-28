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
