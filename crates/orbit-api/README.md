# orbit-api

Manual instrumentation for [Orbit](https://github.com/pierricgimmig/orbit):
scopes, async spans, instants, links and values that a running
`orbit-service` draws on its live timeline.

```rust
let _frame = orbit_api::scope("frame");
orbit_api::value("fps", 59.9);
```

Every call is safe from any thread at any time and is a single predictable
branch until a service is capturing the process. The producer is a lock-free
shared-memory ring: one `fetch_add`, a few plain stores and one release store,
about fifteen nanoseconds with the clock read.

This crate is also the one implementation behind every other language:

- **C and C++** include `include/orbit.h`, a single header that links nothing
  and loads `liborbit_api` at run time (the `cdylib` this crate builds).
- **Python** uses the `orbit-api` package on PyPI, pure Python over the same
  library.

See the header for the C API and its search order, and the project manual
for what reaches the timeline.
