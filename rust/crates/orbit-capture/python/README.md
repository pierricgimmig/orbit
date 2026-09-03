# Reading an Orbit capture in Python

Orbit's live viewer saves a capture as an Arrow IPC file — the **Save** pill,
or `GET /api/capture/export`. It is a plain Arrow file, so any Arrow reader
opens it and it drops straight into pandas; nothing Orbit-specific to install.
The same endpoint hands out Parquet with `?format=parquet`, and the service's
command line can write a whole capture as a dataset directory with
`orbit-service --out-arrow <dir>`.

```bash
pip install pyarrow          # pandas is optional, for the DataFrame view
python open_capture.py capture.arrow      # an Arrow IPC file
python open_capture.py capture.parquet    # a Parquet file
python open_capture.py my-capture/        # a dataset directory
```

The one thing to know before summing numbers: two event kinds reuse the
`duration_ns` column for something that is **not** a duration —

- `VALUE` (6) stashes a float in the bits (`f32::from_bits`); `open_capture.py`
  has `value_of()` to unpack it.
- `SAMPLE` (7) stores the sampling period, not a measured span.

So filter to `API_SCOPE` / `FUNCTION_CALL` (and `SCHEDULING_SLICE`) before you
add durations up. Every column, and the full kind table, is documented at the
top of `open_capture.py`.

## One file: Arrow IPC or Parquet

The smallest thing that works, once you have the file:

```python
import pyarrow.ipc as ipc, pyarrow as pa
table = ipc.open_file(pa.memory_map("capture.arrow")).read_all()
df = table.to_pandas()          # one row per event, name already resolved
scopes = df[df.kind == 1]       # API_SCOPE
print(scopes.groupby("name").duration_ns.sum().sort_values().tail(10) / 1e6)
```

Parquet is the same table in the other container, so pandas can read it
directly:

```python
import pandas as pd
df = pd.read_parquet("capture.parquet")   # needs pyarrow installed
```

Both files are written in batches of 65,536 rows (Arrow record batches /
Parquet row groups), so a reader can stream a large capture batch by batch —
`ipc.open_file(...).get_batch(i)` or `pq.ParquetFile(...).iter_batches()` —
rather than loading it whole.

## A dataset directory

`orbit-service --out-arrow <dir>` writes the full capture as three tables plus
a manifest:

```
my-capture/
  manifest.json    format version, row counts, capture time bounds, file names
  events.arrow     the events table above (scheduling slices on this path)
  samples.arrow    one row per sampled callstack: timestamp_ns, tid,
                   frames (list<u32>, innermost first — ids into frames.arrow)
  frames.arrow     id, name, module, address — what each frame id means
```

Join samples to frames to get a hottest-leaf list:

```python
import json, pyarrow as pa, pyarrow.ipc as ipc
d = "my-capture"
manifest = json.load(open(f"{d}/manifest.json"))
frames = ipc.open_file(pa.memory_map(f"{d}/frames.arrow")).read_pandas().set_index("id")
samples = ipc.open_file(pa.memory_map(f"{d}/samples.arrow")).read_pandas()
leaf = samples.frames.str[0]                     # innermost frame id per sample
print(frames.loc[leaf.dropna().astype(int)].address.value_counts().head())
```

On the command-line capture path the frames carry addresses but no names
(that path does not symbolize); a capture exported from the running service
resolves names in the events table.

The schemas are defined in `../src/lib.rs` (`events_schema`, `samples_schema`,
`frames_schema`), which are the source of truth if a column ever changes.
