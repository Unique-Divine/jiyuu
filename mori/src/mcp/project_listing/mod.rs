//! Own the list_projects request while borrowing native directory and store operations.

use std::fmt;
use std::ops::Range;

use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;

#[cfg(feature = "native-engine-abi")]
mod native;

struct ListOptions {
    metadata_only: bool,
    include_stats: bool,
    wants_json: bool,
    limit: i32,
    offset: i32,
}

impl Default for ListOptions {
    fn default() -> Self {
        Self {
            metadata_only: false,
            include_stats: false,
            wants_json: false,
            limit: 50,
            offset: 0,
        }
    }
}

impl<'de> Deserialize<'de> for ListOptions {
    fn deserialize<D: Deserializer<'de>>(input: D) -> Result<Self, D::Error> {
        struct OptionsVisitor;

        impl<'de> Visitor<'de> for OptionsVisitor {
            type Value = ListOptions;

            fn expecting(
                &self,
                formatter: &mut fmt::Formatter<'_>,
            ) -> fmt::Result {
                formatter.write_str("list_projects arguments")
            }

            fn visit_map<M: MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Self::Value, M::Error> {
                let mut options = ListOptions::default();
                let mut detail_stats = false;
                let mut include_details = false;
                #[derive(Clone, Copy, Deserialize)]
                #[serde(rename_all = "snake_case")]
                enum Key {
                    MetadataOnly,
                    Detail,
                    IncludeDetails,
                    Format,
                    Limit,
                    Offset,
                    #[serde(other)]
                    Unknown,
                }
                let mut seen = [false; Key::Unknown as usize + 1];

                while let Some((key, value)) =
                    map.next_entry::<Key, &RawValue>()?
                {
                    // yyjson_obj_get uses the first matching key regardless of type.
                    if std::mem::replace(&mut seen[key as usize], true) {
                        continue;
                    }
                    let integer = || value.get().parse::<i32>().ok();
                    let text_is = |expected| {
                        // Native comparisons stop at the first encoded NUL.
                        serde_json::from_str::<String>(value.get()).is_ok_and(
                            |text| text.split('\0').next() == Some(expected),
                        )
                    };
                    match key {
                        Key::MetadataOnly => {
                            options.metadata_only = value.get() == "true"
                        }
                        Key::Detail => detail_stats = text_is("stats"),
                        Key::IncludeDetails => {
                            include_details = value.get() == "true"
                        }
                        Key::Format => options.wants_json = text_is("json"),
                        Key::Limit => {
                            options.limit = integer().unwrap_or(options.limit)
                        }
                        Key::Offset => {
                            options.offset = integer().unwrap_or(options.offset)
                        }
                        Key::Unknown => {}
                    }
                }
                options.include_stats =
                    !options.metadata_only && (detail_stats || include_details);
                Ok(options)
            }
        }

        input.deserialize_map(OptionsVisitor)
    }
}

impl ListOptions {
    fn decode(arguments: &[u8]) -> Self {
        let mut result =
            serde_json::from_slice::<Self>(arguments).unwrap_or_default();
        result.limit = result.limit.clamp(1, 500);
        result.offset = result.offset.max(0);
        result
    }

    fn page(&self, count: usize) -> Range<usize> {
        let start = (self.offset as usize).min(count);
        start..(start + self.limit as usize).min(count)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListError {
    Cancelled,
    Collect,
    Populate,
}

trait ProjectRecord {
    fn sort_key(&self) -> (&[u8], &[u8], &[u8]);
}

trait ProjectCatalog {
    type Record: ProjectRecord;

    fn next_record(&mut self) -> Result<Option<Self::Record>, ListError>;
    fn hydrate(
        &mut self,
        record: &mut Self::Record,
        include_stats: bool,
    ) -> Result<(), ListError>;
}

fn collect_projects<C: ProjectCatalog>(
    catalog: &mut C,
    options: &ListOptions,
) -> Result<Vec<C::Record>, ListError> {
    let mut records = Vec::new();
    while let Some(record) = catalog.next_record()? {
        if records.len() == i32::MAX as usize {
            return Err(ListError::Collect);
        }
        records.try_reserve(1).map_err(|_| ListError::Collect)?;
        records.push(record);
    }
    records
        .sort_unstable_by(|left, right| left.sort_key().cmp(&right.sort_key()));
    if !options.metadata_only {
        let page = options.page(records.len());
        for record in &mut records[page] {
            catalog.hydrate(record, options.include_stats)?;
        }
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Record(&'static [u8], &'static [u8], &'static [u8]);

    impl ProjectRecord for Record {
        fn sort_key(&self) -> (&[u8], &[u8], &[u8]) {
            (self.0, self.1, self.2)
        }
    }

    #[derive(Default)]
    struct Catalog {
        records: VecDeque<Result<Record, ListError>>,
        hydrations: Vec<(Record, bool)>,
        hydration_error: Option<ListError>,
    }

    impl ProjectCatalog for Catalog {
        type Record = Record;

        fn next_record(&mut self) -> Result<Option<Record>, ListError> {
            self.records.pop_front().transpose()
        }

        fn hydrate(
            &mut self,
            record: &mut Record,
            include_stats: bool,
        ) -> Result<(), ListError> {
            self.hydrations.push((*record, include_stats));
            self.hydration_error.map_or(Ok(()), Err)
        }
    }

    #[test]
    fn list_options_preserve_first_keys_aliases_and_integer_bounds() {
        let options = ListOptions::decode(
            br#"{"metadata_only":false,"metadata_only":true,"include_details":true,"limit":0,"limit":80,"offset":-1,"format":"json\u0000suffix"}"#,
        );
        assert!(!options.metadata_only);
        assert!(options.include_stats);
        assert!(options.wants_json);
        assert_eq!((options.limit, options.offset), (1, 0));

        let options = ListOptions::decode(
            br#"{"metadata_only":true,"detail":"stats","limit":2147483649,"offset":1.0}"#,
        );
        assert!(options.metadata_only);
        assert!(!options.include_stats);
        assert_eq!((options.limit, options.offset), (50, 0));
        assert_eq!(ListOptions::decode(br#"{"limit":501}"#).limit, 500);
        assert_eq!(ListOptions::decode(br#"{"limit":-0}"#).limit, 1);
        assert_eq!(
            ListOptions::decode(br#"{"unused":{},"limit":12}"#).limit,
            12
        );
        assert_eq!(ListOptions::decode(b"invalid").limit, 50);
    }

    #[test]
    fn sorts_all_identity_bytes_then_hydrates_only_the_requested_page() {
        let early = Record(b"first", b"z", b"z.db");
        let first = Record(b"same", b"root", b"a.db");
        let second = Record(b"same", b"root", b"b.db");
        let third = Record(b"same", b"root\xff", b"0.db");
        let mut catalog = Catalog {
            records: [third, second, first, early].into_iter().map(Ok).collect(),
            ..Catalog::default()
        };
        let options =
            ListOptions::decode(br#"{"detail":"stats","limit":2,"offset":1}"#);
        let records = collect_projects(&mut catalog, &options).unwrap();
        assert_eq!(records, [early, first, second, third]);
        assert_eq!(catalog.hydrations, [(first, true), (second, true)]);
        assert_eq!(options.page(0), 0..0);
    }

    #[test]
    fn metadata_only_skips_hydration_and_cancel_never_returns_a_partial_page() {
        let record = Record(b"test", b"root", b"x.db");
        let mut catalog = Catalog {
            records: [Ok(record)].into(),
            ..Catalog::default()
        };
        let metadata = ListOptions::decode(br#"{"metadata_only":true}"#);
        assert_eq!(collect_projects(&mut catalog, &metadata).unwrap(), [record]);
        assert!(catalog.hydrations.is_empty());

        catalog.records = [Ok(record), Err(ListError::Cancelled)].into();
        assert_eq!(
            collect_projects(&mut catalog, &metadata),
            Err(ListError::Cancelled)
        );
        assert!(catalog.hydrations.is_empty());

        for failure in [ListError::Cancelled, ListError::Populate] {
            catalog.records = [Ok(record)].into();
            catalog.hydration_error = Some(failure);
            assert_eq!(
                collect_projects(&mut catalog, &ListOptions::default()),
                Err(failure)
            );
        }
    }
}
