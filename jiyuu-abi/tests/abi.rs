use alloy_primitives::{Selector, b256, hex};
use alloy_sol_types::{SolCall, sol};
use jiyuu_abi::{
    events_for_topic, extract_abi, functions_for_selector, inspect_abi,
    read_abi, read_abi_file,
};

sol! { function transfer(address to, uint256 amount) external returns (bool); }

#[test]
fn transfer_selector_and_calldata_match_known_vector() {
    let abi = inspect_abi(r#"[{"type":"function","name":"transfer","inputs":[{"name":"to","type":"address"},{"name":"amount","type":"uint256"}],"outputs":[{"type":"bool"}],"stateMutability":"nonpayable"}]"#).unwrap();
    let selector = Selector::from(hex!("a9059cbb"));
    assert_eq!(
        functions_for_selector(&abi, selector)[0].signature(),
        "transfer(address,uint256)"
    );
    let calldata = transferCall {
        to: alloy_primitives::address!(
            "0000000000000000000000000000000000000001"
        ),
        amount: alloy_primitives::U256::from(42u64),
    }
    .abi_encode();
    assert_eq!(
        hex::encode(calldata),
        concat!(
            "a9059cbb",
            "0000000000000000000000000000000000000000000000000000000000000001",
            "000000000000000000000000000000000000000000000000000000000000002a"
        )
    );
}

#[test]
fn overloads_collisions_tuples_events_and_errors() {
    let text = r#"[
      {"type":"function","name":"burn","inputs":[{"type":"uint256"}],"outputs":[]},
      {"type":"function","name":"collate_propagate_storage","inputs":[{"type":"bytes16"}],"outputs":[]},
      {"type":"function","name":"burn","inputs":[{"type":"address"}],"outputs":[]},
      {"type":"function","name":"submit","inputs":[{"type":"tuple","components":[{"type":"address"},{"type":"tuple[]","components":[{"type":"uint256"},{"type":"string"}]}]}],"outputs":[]},
      {"type":"event","name":"Transfer","anonymous":false,"inputs":[{"type":"address","indexed":true},{"type":"address","indexed":true},{"type":"uint256","indexed":false}]},
      {"type":"event","name":"Hidden","anonymous":true,"inputs":[]},
      {"type":"error","name":"Denied","inputs":[{"type":"address"}]}
    ]"#;
    let abi = inspect_abi(text).unwrap();
    let collisions =
        functions_for_selector(&abi, Selector::from(hex!("42966c68")));
    assert_eq!(collisions.len(), 2);
    assert!(collisions.iter().any(|f| f.signature() == "burn(uint256)"));
    assert_eq!(abi.functions.get("burn").unwrap().len(), 2);
    let nested = functions_for_selector(&abi, Selector::from(hex!("51fa5501")));
    assert_eq!(
        nested[0].signature(),
        "submit((address,(uint256,string)[]))"
    );
    let topic = b256!(
        "ddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"
    );
    assert_eq!(events_for_topic(&abi, topic)[0].name, "Transfer");
    let hidden = abi.events().find(|e| e.name == "Hidden").unwrap();
    assert!(events_for_topic(&abi, hidden.selector()).is_empty());
    let error = abi.errors().next().unwrap();
    assert_eq!(
        jiyuu_abi::errors_for_selector(&abi, error.selector())[0].signature(),
        "Denied(address)"
    );
}

#[test]
fn array_and_artifact_bytes_and_files_match_current_importer() {
    let array =
        " \n[ { \"type\": \"function\", \"name\": \"f\", \"inputs\": [] } ]\n";
    let artifact = r#"{"contractName":"Example","abi":[{"type":"function","name":"f","inputs":[]}],"bytecode":"0x1234"}"#;
    let expected = r#"[{"inputs":[],"name":"f","type":"function"}]"#;
    assert_eq!(read_abi(array).unwrap(), array);
    assert_eq!(read_abi(artifact).unwrap(), expected);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("artifact.json");
    std::fs::write(&path, artifact).unwrap();
    assert_eq!(read_abi_file(&path).unwrap(), expected);
    assert_eq!(read_abi(path.to_str().unwrap()).unwrap(), expected);
    std::fs::write(&path, array).unwrap();
    assert_eq!(read_abi_file(&path).unwrap(), array);
}

#[test]
fn malformed_inputs_fail_and_inspection_is_separate() {
    for value in ["[", "{}", r#"{"abi":{}}"#, "null"] {
        assert!(extract_abi(value.into()).is_err(), "{value}");
    }
    // Extraction preserves legacy accepted payloads; typed inspection is stricter.
    let value = extract_abi(r#"[{"type":"not-a-kind"}]"#.into()).unwrap();
    assert!(inspect_abi(&value).is_err());
    assert!(extract_abi(format!("[\"{}\"]", "x".repeat(512 * 1024))).is_err());
}

#[test]
fn characterize_alloy_type_validation() {
    let tuple = r#"[{"type":"function","name":"f","inputs":[{"type":"tuple[abc]","components":[{"type":"uint256"}]}],"outputs":[]}]"#;
    // Alloy 1.7.3 accepts a malformed tuple dimension. Do not mistake parsing
    // for complete type validation when implementing the import boundary.
    assert!(inspect_abi(tuple).is_ok());
}

#[test]
fn validation_covers_types_and_empty_tuples() {
    for ty in [
        "uint257",
        "uint7",
        "bytes33",
        "tuple[abc]",
        "tuple[1]tail",
        "fixed7x1",
        "ufixed128x81",
    ] {
        let components = if ty.starts_with("tuple") {
            r#", "components":[]"#
        } else {
            ""
        };
        let input = format!(
            r#"[{{"type":"function","name":"f","inputs":[{{"type":"{ty}"{components}}}],"outputs":[]}}]"#
        );
        assert!(jiyuu_abi::validate_abi(&input).is_err(), "{ty}");
    }
    for input in [
        "[]",
        r#"[{"type":"function","name":"f","inputs":[{"type":"tuple[]","components":[]}],"outputs":[]}]"#,
    ] {
        jiyuu_abi::validate_abi(input).unwrap();
    }
    for ty in ["fixed", "ufixed128x18", "byte"] {
        let input = format!(
            r#"[{{"type":"function","name":"f","inputs":[{{"type":"{ty}"}}],"outputs":[]}}]"#
        );
        jiyuu_abi::validate_abi(&input)
            .unwrap_or_else(|e| panic!("{ty}: {e:#}"));
    }
    for param in [
        r#"{"type":"uint256[0]"}"#,
        r#"{"type":"tuple[0]","components":[]}"#,
        r#"{"type":"tuple[0]","components":[{"type":"uint256"}]}"#,
    ] {
        let input = format!(
            r#"[{{"type":"function","name":"f","inputs":[{param}],"outputs":[]}}]"#
        );
        jiyuu_abi::validate_abi(&input).unwrap();
        assert_eq!(extract_abi(input.clone()).unwrap(), input);
    }
}

#[test]
fn validation_enforces_event_topic_limits() {
    for (anonymous, count, valid) in [
        (false, 3, true),
        (false, 4, false),
        (true, 4, true),
        (true, 5, false),
    ] {
        let mut inputs =
            vec![serde_json::json!({"type":"address", "indexed":true}); count];
        inputs.push(serde_json::json!({"type":"uint256", "indexed":false}));
        let abi = serde_json::json!([{"type":"event", "name":"Changed", "anonymous":anonymous, "inputs":inputs}]).to_string();
        assert_eq!(
            jiyuu_abi::validate_abi(&abi).is_ok(),
            valid,
            "anonymous={anonymous}, indexed={count}"
        );
    }
}

#[test]
fn canonical_aliases_and_empty_tuples_match_cast_vectors() {
    let raw = r#"[
      {"type":"function","name":"f","inputs":[{"type":"uint"}],"outputs":[]},
      {"type":"function","name":"f","inputs":[{"type":"tuple[]","components":[{"type":"uint"},{"type":"int"}]}],"outputs":[]},
      {"type":"function","name":"empty","inputs":[{"type":"tuple[]","components":[]}],"outputs":[]},
      {"type":"function","name":"enumValue","inputs":[{"type":"Contract.Enum[]"}],"outputs":[]},
      {"type":"error","name":"Oops","inputs":[{"type":"uint"}]},
      {"type":"event","name":"Updated","inputs":[{"type":"uint","indexed":false}],"anonymous":false}
    ]"#;
    let abi = inspect_abi(raw).unwrap();
    assert_eq!(
        functions_for_selector(&abi, Selector::from(hex!("b3de648b"))).len(),
        1
    );
    assert_eq!(
        functions_for_selector(&abi, Selector::from(hex!("73ae7c45"))).len(),
        1
    );
    assert_eq!(
        functions_for_selector(&abi, Selector::from(hex!("6e5c2400"))).len(),
        1
    );
    assert_eq!(
        functions_for_selector(&abi, Selector::from(hex!("a0ed3367"))).len(),
        1
    );
    assert_eq!(
        jiyuu_abi::errors_for_selector(&abi, Selector::from(hex!("50966f79")))
            .len(),
        1
    );
    assert_eq!(events_for_topic(&abi, b256!("208509800c5cb9707f116ef96a1d456499ab9fa3c8edc1cdf381fe5216d5b173")).len(), 1);
    let function =
        functions_for_selector(&abi, Selector::from(hex!("b3de648b")))[0];
    assert_eq!(
        jiyuu_abi::function_selector(function),
        Selector::from(hex!("b3de648b"))
    );
    let error =
        jiyuu_abi::errors_for_selector(&abi, Selector::from(hex!("50966f79")))
            [0];
    assert_eq!(
        jiyuu_abi::error_selector(error),
        Selector::from(hex!("50966f79"))
    );
    let topic = b256!(
        "208509800c5cb9707f116ef96a1d456499ab9fa3c8edc1cdf381fe5216d5b173"
    );
    assert_eq!(
        jiyuu_abi::event_topic(events_for_topic(&abi, topic)[0]),
        topic
    );
    assert_eq!(extract_abi(raw.into()).unwrap(), raw);
}

#[test]
fn standalone_fixtures_are_readable_and_preserve_array_text() {
    let array = include_str!("fixtures/erc20.json");
    let artifact = include_str!("fixtures/contract-artifact.json");
    assert_eq!(extract_abi(array.into()).unwrap(), array);
    for input in [array, artifact] {
        let extracted = extract_abi(input.into()).unwrap();
        jiyuu_abi::validate_abi(&extracted).unwrap();
        assert!(!inspect_abi(&extracted).unwrap().is_empty());
    }
}
