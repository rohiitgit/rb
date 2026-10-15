#!/bin/zsh
# Indexes and dumps every target of bench/manifest.tsv with one rb binary, timing each run
# (contract M8a, S7). Outputs go to var/bench/out/<first 12 of the binary's sha256>/:
#   <name>.<mode>.jsonl     the dump scored by `rb-bench score`
#   <label>.meta            per timed run: binary, command, load, free disk, exit, wall, peak RSS
#   <name>.scip.status      `rb status` after a SCIP run (its jobs' outcomes)
#   pycg/<case>.jsonl       each PyCG case's dump (case path with / as __)
#   hardware.txt, doctor.txt  the machine (perf is compared only on one), `rb doctor` at start
# A target's previous outputs are removed before it runs, so a failed run leaves no stale dump.
# Every repository it creates is named bench_<name>_<mode> and removed on every exit path.
# Exit: 0 when every run and dump succeeded, 1 when any failed, 9 when refused (disk, doctor).
# Kinds: stress, microsecend and handkey targets are indexed (modes from the manifest), pycg cases one by
# one, and dataset rows (answer keys) are only fetched.
# Usage: bench/run.sh BIN [name ...]
# Start it detached so it outlives the terminal (lesson L281):
#   nohup bench/run.sh target/release/rb > var/bench/run.log 2>&1 & disown
set -u
ROOT=${BENCH_ROOT:-${0:A:h:h}}
# Run from a copy, so editing bench/run.sh never changes a run in flight (zsh reads a script as
# it goes; lesson 2026-10-13). The copy removes itself when it exits.
if [[ ${BENCH_RUN_COPY:-} != 1 ]]; then
  mkdir -p $ROOT/var/bench
  copy=$ROOT/var/bench/run.$$.sh
  cp ${0:A} $copy
  BENCH_RUN_COPY=1 BENCH_ROOT=$ROOT exec zsh $copy "$@"
fi
SELF=${0:A}
trap 'rm -f $SELF' EXIT
export RB_DATABASE_URL=${RB_DATABASE_URL:-postgresql://rb:rb@127.0.0.1:55433/rb}
export RB_AGE_LOAD_DIR=${RB_AGE_LOAD_DIR:-$ROOT/var/age-load}
export RB_SCIP_WORK_DIR=${RB_SCIP_WORK_DIR:-$ROOT/var/scip}
# Docker is used through its current context; `rb doctor` below proves it answers.
(( $# >= 1 )) || { echo "usage: bench/run.sh BIN [name ...]" >&2; exit 2; }
BIN=${1:A}; shift
[[ -x $BIN ]] || { echo "not an executable: $BIN" >&2; exit 2; }
cd $ROOT

FAILED=0
fail() { echo "FAILED: $*" >&2; FAILED=$(( FAILED + 1 )) }
finish() {
  cd $ROOT
  bench/cleanup.sh 'bench\_%'
  $BIN gc --prefix bench_ > /dev/null
  rm -f $SELF
}

# enough_disk: the 6 GiB floor of every measurement script (M5 D14); checked before each run,
# since SCIP caches grow by gigabytes.
enough_disk() {
  local free=$(df -k $ROOT | tail -1 | awk '{print $4}')
  (( free >= 6 * 1024 * 1024 )) && return 0
  echo "REFUSED: $(( free / 1024 )) MiB free, below the 6 GiB floor" >&2
  return 1
}
enough_disk || exit 9
SHA256=$(shasum -a 256 $BIN | cut -d' ' -f1)
OUT=$ROOT/var/bench/out/${SHA256[1,12]}
mkdir -p $OUT
print -r -- "$(sysctl -n hw.model) $(sysctl -n hw.ncpu) cpus $(sysctl -n hw.memsize) bytes" > $OUT/hardware.txt
$BIN doctor > $OUT/doctor.txt 2>&1 || { echo "REFUSED: rb doctor failed (see $OUT/doctor.txt)" >&2; exit 9 }
bench/fetch.sh "$@" || exit 1
trap finish EXIT

now() { perl -MTime::HiRes=time -e 'printf "%.3f\n", time' }
load() { sysctl -n vm.loadavg | awk '{print $2}' }
# to <secs> cmd...: run the command in its own process group and kill the whole group after
# <secs> (exit 124), so a timeout stops rb itself, not only the /usr/bin/time around it.
to() {
  local s=$1; shift
  perl -e '$SIG{ALRM}=sub{kill 9,-$p;exit 124}; $p=fork; if(!$p){setpgrp(0,0); exec @ARGV} alarm '"$s"'; waitpid $p,0; exit($?>>8)' "$@"
}

# timed <label> <secs> cmd...: one run under /usr/bin/time -l, its facts in <label>.meta.
timed() {
  local label=$1 secs=$2; shift 2
  local meta=$OUT/$label.meta
  {
    print -r -- "label: $label"
    print -r -- "binary_sha256: $SHA256"
    print -r -- "cmd: $*"
    print -r -- "load_start: $(load)"
    print -r -- "free_start_kib: $(df -k $ROOT | tail -1 | awk '{print $4}')"
  } > $meta
  local start=$(now)
  to $secs /usr/bin/time -l "$@" > $OUT/$label.out 2> $OUT/$label.err
  local rc=$?
  local end=$(now)
  {
    print -r -- "exit: $rc"
    print -r -- "wall_s: $(perl -e "printf '%.2f', $end - $start")"
    print -r -- "peak_rss_bytes: $(awk '/maximum resident set size/ {print $1}' $OUT/$label.err)"
    print -r -- "load_end: $(load)"
  } >> $meta
  echo "$label exit=$rc wall=$(grep '^wall_s' $meta | cut -d' ' -f2)s rss=$(grep '^peak_rss' $meta | cut -d' ' -f2)"
  (( rc == 0 )) || fail "$label exited $rc (see $OUT/$label.err)"
  return $rc
}

dump() { $BIN dump --repo $1 -o $2 || { fail "dump of $1"; return 1 } }

stress() {
  local name=$1 modes=$2 dir=$ROOT/var/bench/repos/$1
  local repo
  rm -f $OUT/$name.*
  for mode in ${(s:,:)modes}; do
    enough_disk || exit 9
    repo=bench_${name}_$mode
    case $mode in
      noscip)
        timed $name.noscip-fresh 1200 $BIN index $dir --repo $repo --no-scip --no-cache --force || continue
        dump $repo $OUT/$name.noscip.jsonl || continue
        timed $name.noscip-cached 1200 $BIN index $dir --repo $repo --no-scip --force || continue
        dump $repo $OUT/$name.noscip-cached.jsonl || continue
        if cmp -s $OUT/$name.noscip.jsonl $OUT/$name.noscip-cached.jsonl; then
          echo "$name: fresh and cached dumps identical"
        else
          fail "$name: DETERMINISM: fresh and cached dumps differ"
        fi
        rm $OUT/$name.noscip-cached.jsonl
        ;;
      scip)
        timed $name.scip 7200 $BIN index $dir --repo $repo --force --scip-timeout-secs 3600 || continue
        dump $repo $OUT/$name.scip.jsonl || continue
        $BIN status --repo $repo > $OUT/$name.scip.status 2>&1
        ;;
      *) fail "$name: unknown mode $mode" ;;
    esac
  done
}

# pycg <sha>: every PyCG case (a directory with a callgraph.json) as its own git work tree, with a
# fixed author and date as invariant.sh makes fixtures, indexed --no-scip and dumped. Not timed.
pycg() {
  local base=$ROOT/var/bench/repos/pycg/micro-benchmark/snippets
  local work=$ROOT/var/bench/work/pycg-${1[1,12]} out=$OUT/pycg
  rm -rf $out
  mkdir -p $work $out
  local i=0 ok=0 g d repo
  for g in ${(f)"$(cd $base && find . -name callgraph.json | sed 's|^\./||; s|/callgraph\.json$||' | LC_ALL=C sort)"}; do
    i=$(( i + 1 )); d=$work/${g//\//__}; repo=bench_pycg_$(printf %03d $i)
    if [[ ! -d $d/.git ]]; then
      mkdir -p $d
      (cd $base/$g && find . -name '*.py' | cpio -pdm $d 2> /dev/null)
      git -C $d init -q -b main && git -C $d add -A
      GIT_AUTHOR_DATE=2026-01-01T00:00:00Z GIT_COMMITTER_DATE=2026-01-01T00:00:00Z \
        git -C $d -c user.name=rb -c user.email=rb@x commit -qm case
    fi
    if $BIN index $d --repo $repo --no-scip --no-cache --force > $out/${g//\//__}.log 2>&1 \
      && dump $repo $out/${g//\//__}.jsonl; then
      ok=$(( ok + 1 ))
    else
      fail "pycg $g: index or dump (see $out/${g//\//__}.log)"
    fi
  done
  echo "pycg: $ok of $i cases dumped"
}

want=("$@")
while IFS=$'\t' read -r name kind url sha modes; do
  [[ -z $name || $name == \#* || $name == name ]] && continue
  (( ${#want} )) && (( ! ${want[(Ie)$name]} )) && continue
  case $kind in
    stress | microsecend | handkey) stress $name $modes ;;
    pycg) enough_disk || exit 9; pycg $sha ;;
    dataset) ;;
    *) fail "$name: kind $kind is not known" ;;
  esac
done < bench/manifest.tsv

echo "outputs: $OUT"
echo "failures: $FAILED"
(( FAILED == 0 )) || exit 1
