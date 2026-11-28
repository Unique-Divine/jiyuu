use super::*;

fn entry(
    id: i64,
    name: &[u8],
    path: &[u8],
    words: Fingerprint,
    bands: [u16; BANDS],
) -> Entry {
    Entry {
        id,
        name: Some(name.to_vec()),
        path: Some(path.to_vec()),
        fingerprint: words,
        bands,
    }
}

fn edges_from(entries: Vec<Entry>, source: i64) -> Vec<SimilarEdge> {
    let index = Index::new(entries);
    let pos = index
        .entries
        .iter()
        .position(|entry| entry.id == source)
        .unwrap();
    let mut edges = Vec::new();
    index.collect_edges(pos, &mut HashSet::new(), &mut edges);
    edges
}

#[test]
fn preserves_band_priority_when_the_per_source_budget_is_full() {
    let words = [3; WORDS];
    let mut below = words;
    below[..4].fill(8);
    let mut just_above = words;
    just_above[..3].fill(8);
    let mut entries = vec![
        entry(200, b"a", b"file.py", words, [0; BANDS]),
        entry(20, b"a-below", b"file.py", below, [0; BANDS]),
        entry(21, b"a-other-language", b"file.rb", words, [0; BANDS]),
    ];
    // The filename extension and QN take precedence over ID. The z*
    // candidates share band 0 and consume the budget before b* in band 1.
    for number in 0..11 {
        let mut later_band = [0; BANDS];
        later_band[0] = 1;
        entries.push(entry(
            60 + number,
            format!("b{number:02}").as_bytes(),
            b"file.py",
            words,
            later_band,
        ));
        entries.push(entry(
            number,
            format!("z{number:02}").as_bytes(),
            b"other.py",
            just_above,
            [0; BANDS],
        ));
    }
    entries.reverse();
    let expected: Vec<_> = (0..10)
        .map(|target| SimilarEdge {
            source: 200,
            target,
            agreement: 61,
            same_file: false,
        })
        .collect();
    assert_eq!(edges_from(entries, 200), expected);
}

#[test]
fn skips_overfull_buckets_and_applies_candidate_limit_before_filtering() {
    let src = || entry(1, b"a", b"file.py", [1; WORDS], [1; BANDS]);
    let mut last_band = [0; BANDS];
    last_band[BANDS - 1] = 1;
    let tail = || entry(2, b"z", b"file.py", [1; WORDS], last_band);

    let crowd = |bands: usize| {
        let groups = (0..bands).flat_map(|band| {
            (0..199).map(move |num| {
                let mut bands = [0; BANDS];
                bands[band] = 1;
                let id = (band * 199 + num + 3) as i64;
                entry(
                    id,
                    format!("miss{id:05}").as_bytes(),
                    b"file.rb",
                    [1; WORDS],
                    bands,
                )
            })
        });
        [tail(), src()]
            .into_iter()
            .chain(groups)
            .collect::<Vec<_>>()
    };

    assert!(edges_from(crowd(21), 1).is_empty());
    let expected = [SimilarEdge {
        source: 1,
        target: 2,
        agreement: 64,
        same_file: true,
    }];
    assert_eq!(edges_from(crowd(20), 1), expected);

    let mut entries = crowd(21);
    // A 201st entry makes the first bucket skip all its candidates. The
    // remaining 20 crowded bands fit below the candidate cap.
    let mut first_band = [0; BANDS];
    first_band[0] = 1;
    entries.push(entry(
        50_000,
        b"overflow",
        b"file.py",
        [1; WORDS],
        first_band,
    ));
    assert_eq!(edges_from(entries, 1), expected);
}

#[test]
fn decodes_fixed_width_extractor_fingerprints() {
    let hex =
        "0000000aFFFFFFFF  +0x1fc-0000001".to_owned() + &"a1234xyz".repeat(60);
    let properties = format!("{{\"fp\":\"{hex}\"}}");
    let words = fingerprint(properties.as_bytes()).unwrap();
    assert_eq!(&words[..5], &[10, u32::MAX, 0x1fc, u32::MAX, 0xa1234]);
    assert!(fingerprint(br#"{"fp":"0a"}"#).is_none());
    assert!(fingerprint(br#"{"fp": "0a"}"#).is_none());
}
