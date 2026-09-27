#!/bin/sh
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# Install Orbit's agent skills (skills/*/SKILL.md) into the user-level skill
# directories that local coding agents scan, the way `q skill install` does:
#
#   ~/.agents/skills/<skill>/SKILL.md   shared cross-agent location (Codex, others)
#   ~/.claude/skills/<skill>/SKILL.md   Claude Code personal skills
#   ~/.cursor/skills/<skill>/SKILL.md   Cursor user skills
#   ~/.codex/skills/<skill>/SKILL.md    Codex legacy path (still scanned)
#
#   tools/install/install_skills.sh              # all four
#   tools/install/install_skills.sh claude codex # a subset: agents claude cursor codex
#   tools/install/install_skills.sh --dir DIR    # one directory of your own
#
# Existing files are overwritten, so rerun after a pull to update.
set -eu

say() { printf 'orbit skills: %s\n' "$1" >&2; }
die() { printf 'orbit skills: error: %s\n' "$1" >&2; exit 1; }

root="$(cd "$(dirname "$0")/../.." && pwd)"
source_dir="$root/skills"
[ -d "$source_dir" ] || die "no skills directory at $source_dir"

dirs=""
custom=""
while [ $# -gt 0 ]; do
  case "$1" in
    --dir)
      [ $# -ge 2 ] || die "--dir needs a path"
      custom="$2"
      shift 2
      ;;
    -h|--help)
      sed -n '5,17p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    agents) dirs="$dirs $HOME/.agents/skills"; shift ;;
    claude) dirs="$dirs $HOME/.claude/skills"; shift ;;
    cursor) dirs="$dirs $HOME/.cursor/skills"; shift ;;
    codex)  dirs="$dirs $HOME/.codex/skills"; shift ;;
    all)    dirs="$dirs $HOME/.agents/skills $HOME/.claude/skills $HOME/.cursor/skills $HOME/.codex/skills"; shift ;;
    *) die "unknown target '$1' (expected agents, claude, cursor, codex, all, or --dir DIR)" ;;
  esac
done
if [ -n "$custom" ]; then
  dirs="$dirs $custom"
fi
if [ -z "$dirs" ]; then
  dirs="$HOME/.agents/skills $HOME/.claude/skills $HOME/.cursor/skills $HOME/.codex/skills"
fi

count=0
for skill_file in "$source_dir"/*/SKILL.md; do
  [ -f "$skill_file" ] || die "no SKILL.md files under $source_dir"
  name="$(basename "$(dirname "$skill_file")")"
  for dir in $dirs; do
    mkdir -p "$dir/$name"
    cp "$skill_file" "$dir/$name/SKILL.md"
    say "wrote $dir/$name/SKILL.md"
    count=$((count + 1))
  done
done
say "$count file(s) installed"
