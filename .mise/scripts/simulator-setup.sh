#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
tag="${OPENTRONS_TAG:-v10.0.0}"
cache="$root/.cache/opentrons"
repo="https://github.com/Opentrons/opentrons.git"

if [[ "$tag" == "edge" || "$tag" == "latest" || "$tag" == main || "$tag" == master ]]; then
  printf 'refusing moving ref %s; pin a versioned release tag\n' "$tag" >&2
  exit 1
fi

if [[ -d "$cache/.git" ]]; then
  current="$(git -C "$cache" describe --tags --exact-match 2>/dev/null || true)"
  if [[ "$current" != "$tag" ]]; then
    printf 'checkout at %s is %s, expected %s; run mise run simulator-clean\n' \
      "$cache" "${current:-unknown}" "$tag" >&2
    exit 1
  fi
else
  mkdir -p "$root/.cache"
  git clone --depth 1 --branch "$tag" "$repo" "$cache"
fi

actual="$(git -C "$cache" describe --tags --exact-match)"
if [[ "$actual" != "$tag" ]]; then
  printf 'expected tag %s, checkout is %s\n' "$tag" "$actual" >&2
  exit 1
fi

# uv 0.9+ requires an RFC 3339 cutoff; Opentrons v10.0.0 still ships "1 week".
python3 - "$cache/robot-server/pyproject.toml" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
text = path.read_text()
old = 'exclude-newer = "1 week"\n'
if old in text:
    path.write_text(text.replace(old, "", 1))
PY

cd "$cache/robot-server"
uv sync --frozen --group dev --python 3.12
uv pip uninstall --yes python-can >/dev/null 2>&1 || true
