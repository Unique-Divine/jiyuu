use super::{Definition, OperationKind, Parser, SelectionKind, signature};

fn encode(source: &str) -> Option<String> {
    let bytes = signature(source.as_bytes(), false, usize::MAX, &mut false)?;
    Some(String::from_utf8(bytes).unwrap())
}

#[test]
fn keeps_operation_and_fragment_spans_in_source_order() {
    let source = b"fragment Details on User { id } query Profile { viewer { ...Details ... on Staff { title } } }";
    let mut parser = Parser::new(source);
    let Some(Definition::Fragment {
        name,
        type_condition,
    }) = parser.next_header(true).unwrap()
    else {
        panic!("expected a fragment");
    };
    assert_eq!(&source[name], b"Details");
    assert_eq!(&source[type_condition], b"User");
    parser.selections(|_| {}).unwrap();
    let Some(Definition::Operation {
        kind,
        name,
        shorthand,
    }) = parser.next_header(true).unwrap()
    else {
        panic!("expected an operation");
    };
    assert_eq!(kind, OperationKind::Query);
    assert_eq!(&source[name], b"Profile");
    assert!(!shorthand);
    let mut spreads = Vec::new();
    parser
        .selections(|selection| match selection.kind {
            SelectionKind::FragmentSpread(span)
            | SelectionKind::InlineFragment(span) => {
                spreads.push((selection.depth, &source[span]));
            }
            _ => {}
        })
        .unwrap();
    assert_eq!(spreads, [(1, &b"Details"[..]), (1, &b"Staff"[..])]);
    assert_eq!(parser.next_header(false), Ok(None));
}

#[test]
fn projects_only_direct_field_paths_in_first_seen_order() {
    let document = r#"
        fragment F on User { name }
        query Profile($id: ID) @live {
            __schema { types }
            first: viewer(id: $id) {
                ...F
                ... on User { name }
                id @include(if: true)
                team { members { id } }
            }
            viewer { id email }
            ... on Query { ignored { id } }
            status
        }
        mutation Profile { viewer { id } }
    "#;
    assert_eq!(
        encode(document).as_deref(),
        Some(
            "query|Profile|viewer,viewer.id,viewer.team,viewer.email,status\nmutation|Profile|viewer,viewer.id"
        )
    );
}

#[test]
fn ignores_braces_in_templates_regex_comments_and_strings() {
    let document = r##"
        ${Leading}\n
        query Q {
            appVersion
            ${field} { hidden }
            ${`outer ${' } '}`}
            ${x /* } */ ? /[}/]{1}/g : typeof /}/}
            ${(a) / 2 / (b)}
            viewer(filter: """literal \""" ) } still in block""") { id }
            status # { ignored }
        }
    "##;
    assert_eq!(
        encode(document).as_deref(),
        Some("query|Q|appVersion,viewer,viewer.id,status")
    );
}

#[test]
fn rejects_bad_selections_even_when_they_do_not_project_paths() {
    for document in [
        "query Q { a: }",
        "query Q { root { child { x.y } } }",
        "query Q { ... on T { a = b } }",
        "query Q { a } fragment F on T {",
        "fragment F on T { a }",
        "query Q { a } { b }",
        "query {user} took too long",
        "https://api.example.com/graphql",
        "query Q { a(",
    ] {
        assert_eq!(encode(document), None, "{document}");
    }
}

#[test]
fn retains_fixed_buffer_boundaries_and_shorthand_indicator() {
    let mut shorthand = false;
    assert_eq!(
        signature(b"fragment F on T { a } { root }", true, 40, &mut shorthand),
        None
    );
    assert!(shorthand);
    let sig = signature(
        b"query Q { veryVeryLongPath x } mutation Next { invalid = selection }",
        true,
        17,
        &mut shorthand,
    );
    assert_eq!(sig.as_deref(), Some(&b"query|Q|x"[..]));
    assert!(!shorthand);
    assert_eq!(signature(b"query Q { x }", true, 8, &mut shorthand), None);
}

#[test]
fn retains_name_and_path_limits_without_truncating_paths() {
    let name = "n".repeat(150);
    let limit = "a".repeat(510);
    let long = "b".repeat(511);
    let doc = format!("query {name} {{ {limit} {{ id }} {long} {{ id }} ok }}");
    let expected = format!("query|{}|{limit},ok", "n".repeat(127));
    assert_eq!(encode(&doc), Some(expected));
}

#[test]
fn handles_deep_nesting_and_wide_duplicate_selections() {
    let deep = format!(
        "query Q {{ root {{ child {{ {}leaf{} }} }} }}",
        " nested {".repeat(5000),
        "}".repeat(5000)
    );
    assert_eq!(encode(&deep).as_deref(), Some("query|Q|root,root.child"));

    let fields = (0..4000)
        .map(|index| format!("f{index}"))
        .collect::<Vec<_>>();
    let selection = fields.join(" ");
    let wide = format!("query Q {{ {selection} {selection} }}");
    assert_eq!(encode(&wide), Some(format!("query|Q|{}", fields.join(","))));
}
