//! Read EVM JSON ABI arrays and compiler artifacts, then inspect them with Alloy.
//!
//! Array text is kept byte-for-byte. For artifacts, the `abi` array is serialized
//! with `serde_json`, matching the existing `ud evm` importer. Hash the returned
//! text, not a serialization of [`JsonAbi`], to retain existing artifact IDs.
//!
//! Alloy's JSON parser checks its JSON structure, but is not a complete Solidity
//! ABI specification validator. Parsing does not prove provenance, completeness,
//! correspondence to deployed bytecode, or the validity of every Solidity type.
//! Alloy 1.7 formats some aliases and empty tuples differently from Solidity's
//! canonical selector types. Use this crate's signature and selector helpers
//! instead of calling `signature()` or `selector()` on returned Alloy items.

use std::{io::Read, path::Path};

pub use alloy_json_abi::{Error as AbiError, Event, Function, JsonAbi};
use alloy_primitives::{B256, Selector, keccak256};
use anyhow::{Context, Result, bail, ensure};

pub const MAX_INPUT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_ABI_BYTES: usize = 512 * 1024;

/// Extract an ABI array, preserving raw array text and existing artifact encoding.
pub fn extract_abi(data: String) -> Result<String> {
    if data.len() > MAX_INPUT_BYTES {
        bail!("ABI or compiler artifact exceeds 16 MiB");
    }
    let value: serde_json::Value =
        serde_json::from_str(&data).context("Invalid ABI JSON")?;
    let abi = if value.is_array() {
        data
    } else {
        let array = value
            .get("abi")
            .and_then(serde_json::Value::as_array)
            .context("ABI must be an array or an artifact with an abi array")?;
        let mut extracted = serde_json::Value::Array(array.clone());
        extracted.sort_all_objects();
        serde_json::to_string(&extracted)?
    };
    if abi.len() > MAX_ABI_BYTES {
        bail!("ABI exceeds 512 KiB");
    }
    Ok(abi)
}

/// Read either inline JSON beginning with `[` or `{`, or a UTF-8 file path.
pub fn read_abi(input: &str) -> Result<String> {
    let trimmed = input.trim_start();
    if trimmed.starts_with('[') || trimmed.starts_with('{') {
        extract_abi(input.to_owned())
    } else {
        read_abi_file(Path::new(input))
    }
}

/// Read an ABI or compiler artifact from a UTF-8 file.
pub fn read_abi_file(path: &Path) -> Result<String> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("Cannot read ABI file {}", path.display()))?;
    let mut data = String::new();
    file.take(MAX_INPUT_BYTES as u64 + 1)
        .read_to_string(&mut data)
        .with_context(|| {
            format!("Cannot read ABI file {} as UTF-8", path.display())
        })?;
    extract_abi(data)
}

/// Parse extracted array text for inspection without changing the stored text.
/// Alloy 1.7 cannot inspect some valid zero-length fixed arrays, even though
/// [`validate_abi`] accepts them. This function returns an error in that case.
pub fn inspect_abi(abi: &str) -> Result<JsonAbi> {
    JsonAbi::from_json_str(abi).context("Invalid ABI entries for Alloy")
}

/// Validate new ABI arrays before storage. Existing stored bytes need no
/// revalidation when referenced by their hash.
pub fn validate_abi(abi: &str) -> Result<()> {
    ensure!(abi.len() <= MAX_ABI_BYTES, "ABI exceeds 512 KiB");
    let value: serde_json::Value =
        serde_json::from_str(abi).context("Invalid ABI JSON")?;
    let entries = value.as_array().context("ABI must be an array")?;
    ensure!(entries.len() <= 4096, "ABI has more than 4096 entries");
    // Alloy rejects a zero-sized fixed array even though the ABI spec permits
    // one. Use a temporary copy for structural inspection, never for storage.
    let mut inspection = value.clone();
    normalize_zero_arrays(&mut inspection);
    let inspected = inspect_abi(&serde_json::to_string(&inspection)?)?;
    for event in inspected.events() {
        let indexed = event.inputs.iter().filter(|input| input.indexed).count();
        let max = if event.anonymous { 4 } else { 3 };
        ensure!(
            indexed <= max,
            "Event {} has {indexed} indexed parameters; maximum is {max}",
            event.name
        );
    }
    for entry in entries {
        for key in ["inputs", "outputs"] {
            if let Some(params) = entry.get(key) {
                for param in
                    params.as_array().context("ABI parameters must be arrays")?
                {
                    validate_param(param)?;
                }
            }
        }
    }
    Ok(())
}

fn normalize_zero_arrays(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Array(values) => {
            values.iter_mut().for_each(normalize_zero_arrays)
        }
        serde_json::Value::Object(fields) => {
            if let Some(serde_json::Value::String(ty)) = fields.get_mut("type") {
                *ty = ty.replace("[0]", "[1]");
            }
            fields.values_mut().for_each(normalize_zero_arrays);
        }
        _ => {}
    }
}

fn validate_param(param: &serde_json::Value) -> Result<()> {
    let ty = param
        .get("type")
        .and_then(serde_json::Value::as_str)
        .context("ABI parameter needs a type")?;
    let (base, mut suffix) = ty.find('[').map_or((ty, ""), |i| ty.split_at(i));
    while !suffix.is_empty() {
        let rest = suffix.strip_prefix('[').context("Invalid ABI array type")?;
        let end = rest.find(']').context("Invalid ABI array type")?;
        let size = &rest[..end];
        ensure!(
            size.is_empty()
                || (size.bytes().all(|b| b.is_ascii_digit())
                    && size.parse::<u64>().is_ok()),
            "Invalid ABI array size"
        );
        suffix = &rest[end + 1..];
    }
    if base == "tuple" {
        let components = param
            .get("components")
            .and_then(serde_json::Value::as_array)
            .context("Tuple needs components")?;
        for component in components {
            validate_param(component)?;
        }
    } else {
        ensure!(
            param
                .get("components")
                .and_then(serde_json::Value::as_array)
                .is_none_or(Vec::is_empty),
            "Only tuples may have components"
        );
        if base == "byte" || base == "fixed" || base == "ufixed" {
            // Solidity ABI aliases for bytes1 and fixed128x18/ufixed128x18.
        } else if let Some(spec) = base
            .strip_prefix("ufixed")
            .or_else(|| base.strip_prefix("fixed"))
        {
            let (width, decimals) = spec
                .split_once('x')
                .context("Invalid fixed-point ABI type")?;
            let width: u16 =
                width.parse().context("Invalid fixed-point width")?;
            let decimals: u8 =
                decimals.parse().context("Invalid fixed-point decimals")?;
            ensure!(
                width > 0
                    && width <= 256
                    && width.is_multiple_of(8)
                    && (1..=80).contains(&decimals),
                "Invalid fixed-point ABI type"
            );
        } else {
            alloy_json_abi::parser::TypeSpecifier::parse(base)
                .context("Invalid ABI type")?
                .try_basic_solidity()
                .context("Invalid ABI type")?;
        }
    }
    Ok(())
}

fn canonical_type(ty: &str, components: &[alloy_json_abi::Param]) -> String {
    let (base, suffix) = ty.find('[').map_or((ty, ""), |i| ty.split_at(i));
    let base = match base {
        "tuple" => format!(
            "({})",
            components
                .iter()
                .map(|p| canonical_type(&p.ty, &p.components))
                .collect::<Vec<_>>()
                .join(",")
        ),
        "uint" => "uint256".into(),
        "int" => "int256".into(),
        "fixed" => "fixed128x18".into(),
        "ufixed" => "ufixed128x18".into(),
        "byte" => "bytes1".into(),
        // Alloy accepts the legacy library ABI spelling Contract.Enum as uint8.
        name if name.contains('.') => "uint8".into(),
        _ => base.into(),
    };
    format!("{base}{suffix}")
}

fn signature<'a>(
    name: &str,
    inputs: impl Iterator<Item = (&'a str, &'a [alloy_json_abi::Param])>,
) -> String {
    format!(
        "{}({})",
        name,
        inputs
            .map(|(ty, components)| canonical_type(ty, components))
            .collect::<Vec<_>>()
            .join(",")
    )
}

/// Canonical Solidity signature used to calculate a function selector.
pub fn function_signature(item: &Function) -> String {
    signature(
        &item.name,
        item.inputs
            .iter()
            .map(|p| (p.ty.as_str(), p.components.as_slice())),
    )
}

/// Canonical Solidity signature used to calculate a custom error selector.
pub fn error_signature(item: &AbiError) -> String {
    signature(
        &item.name,
        item.inputs
            .iter()
            .map(|p| (p.ty.as_str(), p.components.as_slice())),
    )
}

/// Canonical Solidity signature used to calculate a non-anonymous event topic.
pub fn event_signature(item: &Event) -> String {
    signature(
        &item.name,
        item.inputs
            .iter()
            .map(|p| (p.ty.as_str(), p.components.as_slice())),
    )
}

fn selector(signature: &str) -> Selector {
    Selector::from_slice(&keccak256(signature.as_bytes())[..4])
}

/// Compute a function selector using canonical parameter types.
pub fn function_selector(item: &Function) -> Selector {
    selector(&function_signature(item))
}

/// Compute a custom error selector using canonical parameter types.
pub fn error_selector(item: &AbiError) -> Selector {
    selector(&error_signature(item))
}

/// Compute an event signature hash. Anonymous events omit it from log topics.
pub fn event_topic(item: &Event) -> B256 {
    keccak256(event_signature(item).as_bytes())
}

/// Return every function with this selector. Four-byte selectors can collide.
pub fn functions_for_selector(
    abi: &JsonAbi,
    selector: Selector,
) -> Vec<&Function> {
    abi.functions()
        .filter(|item| function_selector(item) == selector)
        .collect()
}

/// Return every custom error with this selector. Four-byte selectors can collide.
pub fn errors_for_selector(abi: &JsonAbi, selector: Selector) -> Vec<&AbiError> {
    abi.errors()
        .filter(|item| error_selector(item) == selector)
        .collect()
}

/// Return non-anonymous events with this signature topic. Anonymous events do not
/// include their signature as the first log topic.
pub fn events_for_topic(abi: &JsonAbi, topic: B256) -> Vec<&Event> {
    abi.events()
        .filter(|item| !item.anonymous && event_topic(item) == topic)
        .collect()
}
