# perfr-api

perfr manual instrumentation for Python. Pure Python, no dependencies.

```python
import perfr_api as perfr

perfr.init()                      # finds libperfr_api, or stays quiet

with perfr.scope("update"):
    ...

@perfr.scope                      # or @perfr.scope("a name")
def render(frame):
    ...

perfr.value("fps", 59.9)
perfr.instant("level loaded")
```

Run `perfr`, open its viewer, pick the Python process, press Record.

## How it finds the library

The producer is the same `libperfr_api` that C and C++ load through
`perfr.h`. The package looks, in order, at `$PERFR_API_LIB`, beside the
`perfr` on `PATH` (then `~/.local/bin`, `~/.perfr/bin`), the copy
bundled in this wheel, then the system loader. The service comes first
because it is the authority on the ring protocol version: the library that
writes must match the service that reads, and a running service's own
library is the one guaranteed to. Without any library every call is a no-op
and `init()` returns `perfr_api.E_NOLIB`.

## API

`init() -> int`, `shutdown()`, `available()`, `library_path()`,
`start(name) -> handle`, `start_async(name)`, `stop(handle)`,
`instant(name) -> handle`, `link(src, dst)`, `value(name, v)`,
`now_ns()`, `span(name, start_ns, end_ns)`, `span_async(...)`,
`scope(name)` and `scope_async(name)` as context managers or decorators.

Names may be `str` or `bytes`; `bytes` skips an encode on the hot path.
Handles are ints; `0` means "no event" and every function accepts it.
