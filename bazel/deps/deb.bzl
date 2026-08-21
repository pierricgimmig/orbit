# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""A repository rule that materializes a set of Debian packages as one file tree.

Bazel can extract the outer `ar` container of a `.deb` natively, so this only
has to pick the inner `data.tar.*` member back out and unpack it on top of the
repository root. No host tools are involved, which keeps the rule usable
everywhere Bazel itself runs.

Packages are pinned by URL and SHA-256 in //bazel/deps:debs.bzl, which is
generated from apt metadata by //bazel/deps:resolve_debs.py.
"""

# The compression `data.tar` may use, in the order dpkg prefers them. Bazel
# derives the archive type from the file name, so the suffix has to be kept.
_DATA_MEMBERS = [
    "data.tar.zst",
    "data.tar.xz",
    "data.tar.gz",
    "data.tar.bz2",
    "data.tar",
]

def _extract_deb(ctx, package, index):
    """Downloads one .deb and unpacks its payload into the repository root."""
    staging = "_deb/%d" % index
    archive = "%s/package.deb" % staging

    ctx.download(
        url = package.url,
        output = archive,
        sha256 = package.sha256,
    )

    # First hop: the ar container, which yields debian-binary, control.tar.*
    # and data.tar.*.
    ctx.extract(archive = archive, output = staging)

    for member in _DATA_MEMBERS:
        payload = "%s/%s" % (staging, member)
        if ctx.path(payload).exists:
            # Second hop: the payload, unpacked over the shared tree so that
            # every package in the group lands in one /usr hierarchy.
            ctx.extract(archive = payload, output = "")
            ctx.delete(staging)
            return

    fail("%s: no data member found in %s (looked for %s)" %
         (package.name, package.url, ", ".join(_DATA_MEMBERS)))

def _deb_packages_impl(ctx):
    for index, encoded in enumerate(ctx.attr.packages):
        name, url, sha256 = encoded.split("|")
        _extract_deb(ctx, struct(name = name, url = url, sha256 = sha256), index)

    ctx.delete("_deb")
    ctx.file("WORKSPACE", "workspace(name = \"%s\")\n" % ctx.name)
    ctx.template("BUILD.bazel", ctx.attr.build_file, substitutions = ctx.attr.substitutions)

deb_packages = repository_rule(
    implementation = _deb_packages_impl,
    doc = "Unpacks a group of pinned .deb packages into a single repository.",
    attrs = {
        "packages": attr.string_list(
            mandatory = True,
            doc = "Packages encoded as \"name|url|sha256\", as produced by deb_list().",
        ),
        "build_file": attr.label(
            mandatory = True,
            doc = "BUILD file template describing the unpacked tree.",
        ),
        "substitutions": attr.string_dict(
            doc = "Substitutions applied to build_file.",
        ),
    },
)

def deb_list(packages):
    """Encodes structs from debs.bzl for the `packages` attribute."""
    return ["%s|%s|%s" % (p.name, p.url, p.sha256) for p in packages]
