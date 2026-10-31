# perfr-api

Manual instrumentation for [perfr](https://github.com/perfr/perfr):
scopes, async spans, instants, links and values that a running
`perfr` draws on its live timeline.

```rust
let _frame = perfr_api::scope("frame");
perfr_api::value("fps", 59.9);
```

Every call is safe from any thread at any time and is a single predictable
branch until a service is capturing the process. The producer is a lock-free
shared-memory ring: one `fetch_add`, a few plain stores and one release store,
about fifteen nanoseconds with the clock read.

This crate is also the one implementation behind every other language:

- **C and C++** include `include/perfr.h`, a single header that links nothing
  and loads `libperfr_api` at run time (the `cdylib` this crate builds).
- **Python** uses the `perfr-api` package on PyPI, pure Python over the same
  library.

See the header for the C API and its search order, and the project manual
for what reaches the timeline.
