#!/usr/bin/env bash
# Convenience wrapper so `./install.sh` from a fresh clone just works.
# The real script lives in packaging/; see docs/qdropd.md for details.
exec "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/packaging/install.sh" "$@"
