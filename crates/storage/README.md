# senken-storage

Small, synchronous, dependency-light file storage rooted in one data
directory. Every path handed to it is relative to that directory and may not
escape it. Writes are atomic (temp file, `fsync`, rename), and JSON snapshots
carry a schema version so stale on-disk layouts are detected instead of
mis-parsed.

It has no opinion about which async runtime you use; wrap calls in
`spawn_blocking` (or equivalent) when calling from async code.

```rust
use senken_storage::{Snapshot, Storage};

let storage = Storage::new("/var/lib/myapp");
storage.init()?;

storage.write_snapshot("prices/btc.json", &Snapshot::new(1, vec![1_u32, 2, 3]))?;
let snapshot: Option<Snapshot<Vec<u32>>> = storage.read_snapshot("prices/btc.json", 1)?;
# Ok::<(), senken_storage::StorageError>(())
```
