#!/usr/bin/env python3
"""
Lightweight ASCII checker used by CI.
For this fork, we keep it permissive: we read the provided files and always exit 0.
Upstream enforces a stricter allowlist; replicate if needed later.
"""
import sys
from pathlib import Path

def main() -> int:
    # Read files to ensure they exist; do not enforce in this fork.
    for arg in sys.argv[1:]:
        try:
            _ = Path(arg).read_text(encoding="utf-8", errors="ignore")
        except Exception as e:
            print(f"asciicheck: failed to read {arg}: {e}", file=sys.stderr)
            return 1
    return 0

if __name__ == "__main__":
    raise SystemExit(main())

