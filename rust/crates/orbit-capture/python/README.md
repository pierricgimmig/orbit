# Reading an Orbit capture in Python

Orbit's live viewer saves a capture as an Arrow IPC file — the **Save** pill,
or `GET /api/capture/export`. It is a plain Arrow file, so any Arrow reader
opens it and it drops straight into pandas; nothing Orbit-specific to install.

```bash
pip install pyarrow          # pandas is optional, for the DataFrame view
python open_capture.py capture.arrow
```

The one thing to know before summing numbers: two event kinds reuse the
`duration_ns` column for something that is **not** a duration —

- `VALUE` (6) stashes a float in the bits (`f32::from_bits`); `open_capture.py`
  has `value_of()` to unpack it.
- `SAMPLE` (7) stores the sampling period, not a measured span.

So filter to `API_SCOPE` / `FUNCTION_CALL` (and `SCHEDULING_SLICE`) before you
add durations up. Every column, and the full kind table, is documented at the
top of `open_capture.py`.

The smallest thing that works, once you have the file:

```python
import pyarrow.ipc as ipc, pyarrow as pa
table = ipc.open_file(pa.memory_map("capture.arrow")).read_all()
df = table.to_pandas()          # one row per event, name already resolved
scopes = df[df.kind == 1]       # API_SCOPE
print(scopes.groupby("name").duration_ns.sum().sort_values().tail(10) / 1e6)
```

The schema is defined in `../src/lib.rs` (`events_schema`), which is the source
of truth if a column ever changes.
