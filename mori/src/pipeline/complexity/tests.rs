use super::{CallGraph, MalformedPair, NodeKey, PropertyMode, set_properties};
use std::collections::BTreeMap;

fn key(name: &[u8]) -> NodeKey {
    NodeKey {
        qualified_name: name.to_vec(),
        ..NodeKey::default()
    }
}

#[test]
fn propagates_max_callee_depth_in_canonical_order() {
    // A calls B and C. B and C form a cycle and have depths 2 and 3.
    // Entering B first marks B recursive. A also reaches a separate function
    // through a Module, which passes through its callee's depth.
    for ids in [
        [52, 14, 6, 11, 7, 31, 15],
        [3, 90, 70, 12, 10, 21, 45_000_000_000],
    ] {
        let [a, b, c, module, leaf, explicit, dangling] = ids;
        for reverse in [false, true] {
            let mut nodes: Vec<_> = [
                (a, &b"a"[..], Some(&br#"{"loop_depth":1}"#[..])),
                (b, b"b", Some(br#"{"loop_depth":2}"#)),
                (c, b"c", Some(br#"{"loop_depth":3,"note":"keep"}"#)),
                (module, b"d", None),
                (leaf, b"e", Some(br#"{"loop_depth":4}"#)),
                (explicit, b"f", Some(br#"{"self_recursive":true}"#)),
            ]
            .into();
            // Gather C before B. Sorting just the seeds would leave the
            // cycle flag on C because DFS starts at A.
            let mut edges = [
                (a, c),
                (a, b),
                (b, c),
                (c, b),
                (a, module),
                (module, leaf),
                (leaf, leaf),
                (a, dangling),
            ];
            if reverse {
                nodes.reverse();
                edges.reverse();
            }
            let mut graph = CallGraph::default();
            for (id, name, props) in nodes {
                graph.set_node(id, key(name), props.map(<[_]>::to_vec));
            }
            for (caller, callee) in edges {
                graph.add_call(caller, callee);
            }

            let result: BTreeMap<_, _> = graph
                .updates(PropertyMode::Append)
                .unwrap()
                .into_iter()
                .map(|update| (update.id, update.properties))
                .collect();
            assert_eq!(
                result,
                [
                    (a, &br#"{"loop_depth":1,"transitive_loop_depth":6,"recursive":false}"#[..]),
                    (b, br#"{"loop_depth":2,"transitive_loop_depth":5,"recursive":true}"#),
                    (c, br#"{"loop_depth":3,"note":"keep","transitive_loop_depth":3,"recursive":false}"#),
                    (leaf, br#"{"loop_depth":4,"transitive_loop_depth":4,"recursive":true}"#),
                    (explicit, br#"{"self_recursive":true,"transitive_loop_depth":0,"recursive":true}"#),
                ]
                .into_iter()
                .map(|(id, props)| (id, props.to_vec()))
                .collect()
            );
        }
    }
}

#[test]
fn depth_cutoff_does_not_cache_unvisited_callees() {
    let mut graph = CallGraph::default();
    for id in 1..=260 {
        graph.set_node(
            id,
            key(format!("{id:03}").as_bytes()),
            Some(br#"{"loop_depth":1}"#.to_vec()),
        );
        if id > 1 {
            graph.add_call(id - 1, id);
        }
    }
    graph.add_call(258, 258);
    let updates = graph.updates(PropertyMode::Append).unwrap();
    assert_eq!(
        updates[0].properties,
        br#"{"loop_depth":1,"transitive_loop_depth":258,"recursive":false}"#
    );
    assert_eq!(
        updates[257].properties,
        br#"{"loop_depth":1,"transitive_loop_depth":3,"recursive":true}"#
    );
}

#[test]
fn closure_rewrites_the_existing_pair_and_preserves_other_bytes() {
    let old = b"{\"loop_depth\":2,\"label\":\"\xff\",\"transitive_loop_depth\":-10,\"recursive\":true,\"importance\":1.250}";
    let new = b"{\"loop_depth\":2,\"label\":\"\xff\",\"transitive_loop_depth\":3,\"recursive\":false,\"importance\":1.250}";
    assert_eq!(
        set_properties(old, 3, false, PropertyMode::Rewrite),
        Ok(Some(new.to_vec()))
    );
    assert_eq!(
        set_properties(new, 3, false, PropertyMode::Rewrite),
        Ok(Some(new.to_vec()))
    );
    assert_eq!(
        set_properties(b"{}", 0, false, PropertyMode::Rewrite),
        Ok(Some(
            br#"{"transitive_loop_depth":0,"recursive":false}"#.to_vec()
        ))
    );
    assert_eq!(
        set_properties(b"null", 0, false, PropertyMode::Append),
        Ok(None)
    );
    assert_eq!(
        set_properties(
            br#"{"transitive_loop_depth":10,"other":true}"#,
            3,
            false,
            PropertyMode::Rewrite
        ),
        Err(MalformedPair)
    );
}
