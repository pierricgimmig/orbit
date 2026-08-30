# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Runs one C++ test suite against both the C++ and the Rust implementation.

The port keeps both implementations in the tree and selects between them with
an environment variable. This macro emits three cc_test targets from a single
set of attributes, so a suite cannot drift between backends:

    <name>       the suite as it has always run, backend "cpp"
    <name>Rust   the same assertions, with Rust doing the work
    <name>Both   runs both implementations and aborts if they disagree

Only `env` differs between them. Everything else -- srcs, deps, data, copts,
size -- is shared by construction.

See docs/rust-port-plan.html.
"""

load("@rules_cc//cc:defs.bzl", "cc_test")

# suffix -> value of the backend environment variable.
_BACKENDS = [
    ("", "cpp"),
    ("Rust", "rust"),
    ("Both", "both"),
]

def orbit_dual_backend_test(name, backend_env_var, env = None, tags = None, **kwargs):
    """Emits <name>, <name>Rust and <name>Both.

    Args:
      name: name of the C++-backend target; the others append a suffix.
      backend_env_var: environment variable the code under test reads, e.g.
        "ORBIT_MAPS_BACKEND".
      env: shared environment, copied into each target before the backend
        variable is added.
      tags: shared tags; each target additionally gets "orbit-backend-<value>"
        so a whole backend can be selected with --test_tag_filters.
      **kwargs: forwarded to every cc_test unchanged.
    """
    base_env = dict(env) if env else {}
    base_tags = list(tags) if tags else []

    for suffix, backend in _BACKENDS:
        target_env = dict(base_env)
        target_env[backend_env_var] = backend

        cc_test(
            name = name + suffix,
            env = target_env,
            tags = base_tags + ["orbit-backend-" + backend],
            **kwargs
        )
