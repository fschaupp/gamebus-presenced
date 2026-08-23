#!/usr/bin/env sh
# Check the gamedb lint still reports exactly what it is meant to.
#
# The fixture set carries one page per rule the lint enforces, plus pages that
# must stay silent (a title shared by two different games, a numeric store id
# used as a canonical id). expected.txt is the whole report, warnings included.
set -eu
here=$(dirname "$0")
"$here/gamedb-lint.py" "$here/gamedb-lint-fixtures" > /tmp/gamedb-lint-actual.txt 2>&1 || true
if diff -u "$here/gamedb-lint-fixtures/expected.txt" /tmp/gamedb-lint-actual.txt; then
	echo "gamedb lint self-test: OK"
else
	echo "gamedb lint self-test: report changed" >&2
	exit 1
fi
