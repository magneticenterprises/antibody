#!/usr/bin/env sh
# Points git at the versioned hooks in .husky/ (no husky package needed).
set -eu
cd "$(git rev-parse --show-toplevel)"
chmod +x .husky/pre-commit .husky/commit-msg .husky/pre-push
git config core.hooksPath .husky
echo "git hooks installed from .husky/"
