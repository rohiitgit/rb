#!/bin/zsh
# Clones every target of bench/manifest.tsv at its pinned commit into var/bench/repos/<name> and
# verifies the pin (contract M8a, S7). Idempotent: a clone already at its pin is kept; a clone at
# another commit is an error (remove it by hand). When a local clone already sits at the pin (the
# stress campaign's var/stress/repos), it is the fetch source, which saves the download.
# Usage: bench/fetch.sh [name ...]
set -eu
ROOT=${0:A:h:h}
DEST=$ROOT/var/bench/repos
mkdir -p $DEST
want=("$@")
while IFS=$'\t' read -r name kind url sha modes; do
  [[ -z $name || $name == \#* || $name == name ]] && continue
  (( ${#want} )) && (( ! ${want[(Ie)$name]} )) && continue
  d=$DEST/$name
  if [[ -d $d/.git ]]; then
    head=$(git -C $d rev-parse HEAD)
    [[ $head == $sha ]] || { echo "$name: $d is at $head, not the pin $sha; remove it by hand" >&2; exit 1; }
    echo "$name ok ${sha[1,12]}"
    continue
  fi
  src=$url; ref=$sha
  for s in $ROOT/var/stress/repos/*(/N); do
    if [[ $(git -C $s rev-parse HEAD 2>/dev/null) == $sha ]]; then src=$s; ref=HEAD; break; fi
  done
  git init -q -b main $d
  git -C $d remote add origin $url
  git -C $d fetch -q --depth 1 $src $ref
  git -C $d checkout -q --detach FETCH_HEAD
  [[ $(git -C $d rev-parse HEAD) == $sha ]] || { echo "$name: PIN MISMATCH after fetch from $src" >&2; exit 1; }
  echo "$name fetched ${sha[1,12]} from $src: $(git -C $d ls-files | wc -l | tr -d ' ') files, $(du -sh $d | cut -f1)"
done < $ROOT/bench/manifest.tsv
