# jiyuu-abi

Rust utilities for EVM JSON ABIs and compiler artifacts. Accept an ABI array or
an artifact with an `abi` array, validate new ABI data, inspect functions and
events, and match selectors or event topics.

```rust
let text = jiyuu_abi::read_abi("./abi.json")?;
jiyuu_abi::validate_abi(&text)?;
let abi = jiyuu_abi::inspect_abi(&text)?;
# Ok::<(), anyhow::Error>(())
```

Raw ABI arrays retain their original bytes. Artifact ABI arrays are serialized
with sorted object keys. Keep the extracted text when hashing or storing it;
serializing Alloy's parsed representation can change that text.

Validation checks structure and types, but cannot prove that an ABI matches
deployed bytecode. Alloy 1.7 cannot inspect some valid zero-length fixed arrays,
although validation accepts them. Use this crate's selector and topic helpers
for canonical aliases, and handle every result because four-byte selectors can
collide.
