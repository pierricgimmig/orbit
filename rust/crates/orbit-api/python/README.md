# orbit-api

Orbit manual instrumentation for Python. Pure Python, no dependencies.

```python
import orbit_api as orbit

orbit.init()                      # finds liborbit_api, or stays quiet

with orbit.scope("update"):
    ...

@orbit.scope                      # or @orbit.scope("a name")
def render(frame):
    ...

orbit.value("fps", 59.9)
orbit.instant("level loaded")
```

Run `orbit-service`, open its viewer, pick the Python process, press Record.

## How it finds the library

The producer is the same `liborbit_api` that C and C++ load through
`orbit.h`. The package looks, in order, at `$ORBIT_API_LIB`, beside the
`orbit-service` on `PATH` (then `~/.local/bin`, `~/.orbit/bin`), the copy
bundled in this wheel, then the system loader. The service comes first
because it is the authority on the ring protocol version: the library that
writes must match the service that reads, and a running service's own
library is the one guaranteed to. Without any library every call is a no-op
and `init()` returns `orbit_api.E_NOLIB`.

## API

`init() -> int`, `shutdown()`, `available()`, `library_path()`,
`start(name) -> handle`, `start_async(name)`, `stop(handle)`,
`instant(name) -> handle`, `link(src, dst)`, `value(name, v)`,
`now_ns()`, `span(name, start_ns, end_ns)`, `span_async(...)`,
`scope(name)` and `scope_async(name)` as context managers or decorators.

Names may be `str` or `bytes`; `bytes` skips an encode on the hot path.
Handles are ints; `0` means "no event" and every function accepts it.
