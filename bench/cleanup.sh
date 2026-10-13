#!/bin/zsh
# Removes the repositories whose names match a SQL LIKE pattern, with their AGE graphs, one graph
# per transaction (rb has no command that deletes a repository). Never touches anything else.
# Ported from var/m5/scripts/cleanup.sh. Usage: bench/cleanup.sh 'bench\_%'
set -u
(( $# == 1 )) || { echo "usage: bench/cleanup.sh PATTERN" >&2; exit 2; }
PAT=$1
[[ $PAT == bench\\_* ]] || { echo "refusing a pattern outside bench\\_%: $PAT" >&2; exit 2; }
URL=${RB_DATABASE_URL:-postgresql://rb:rb@127.0.0.1:55433/rb}
q() { psql "$URL" -v ON_ERROR_STOP=1 -tAq -c "$1" }
for r in ${(f)"$(q "SELECT name FROM rb.repo WHERE name LIKE '$PAT' ORDER BY name")"}; do
  [[ -z $r ]] && continue
  for g in ${(f)"$(q "SELECT s.age_graph FROM rb.snapshot s JOIN rb.repo r ON r.id = s.repo_id WHERE r.name = '$r' AND s.age_graph IS NOT NULL")"}; do
    [[ -n $g ]] && q "SET client_min_messages = warning; SET search_path = ag_catalog, public; SELECT drop_graph('$g', true);" > /dev/null
  done
  q "UPDATE rb.repo SET live_snapshot_id = NULL WHERE name = '$r'; DELETE FROM rb.repo WHERE name = '$r';"
done
echo "repositories left like $PAT: $(q "SELECT count(*) FROM rb.repo WHERE name LIKE '$PAT'")"
