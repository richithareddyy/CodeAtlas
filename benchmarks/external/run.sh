#!/usr/bin/env bash
# Reproduces the external test-selection evaluation (see PROTOCOL.md).
#
#   run.sh setup               image, clones at the pinned commits, lockfiles,
#                              dependency downloads, Linux codeatlas build
#   run.sh probe NAME SET      mutation probes in a container without network
#   run.sh compare NAME SET    strategy comparison (static, on the host)
#
# SET is `primary` or `held_out` (seeds and sizes in repos.json). Clones and
# caches live in $CODEATLAS_EVAL_WORK (default ~/.cache/codeatlas-eval).
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
work="${CODEATLAS_EVAL_WORK:-$HOME/.cache/codeatlas-eval}"
image=codeatlas-eval
cargo_volume=codeatlas-eval-cargo
build_volume=codeatlas-eval-build

field() { python3 -c "import json,sys; r=json.load(open('$here/repos.json')); print(eval(sys.argv[1]))" "$1"; }
names() { field "' '.join(x['name'] for x in r['repositories'])"; }
repo_field() { field "[x for x in r['repositories'] if x['name']=='$1'][0]['$2']"; }

container() {
    docker run --rm -v "$cargo_volume:/cargo-home" -v "$build_volume:/build" "$@"
}

setup() {
    mkdir -p "$work/repos" "$here/locks"
    docker build -t "$image" "$here"
    for name in $(names); do
        local dir="$work/repos/$name" commit
        commit="$(repo_field "$name" commit)"
        if [ ! -d "$dir/.git" ]; then
            git clone --quiet "$(repo_field "$name" url)" "$dir"
        fi
        git -C "$dir" checkout --quiet --detach "$commit"
        if git -C "$dir" ls-files --error-unmatch Cargo.lock >/dev/null 2>&1; then
            echo "$name: committed Cargo.lock"
        elif [ -f "$here/locks/$name.Cargo.lock" ]; then
            cp "$here/locks/$name.Cargo.lock" "$dir/Cargo.lock"
        else
            container -v "$dir:/repo" -w /repo "$image" cargo generate-lockfile
            cp "$dir/Cargo.lock" "$here/locks/$name.Cargo.lock"
            echo "$name: generated locks/$name.Cargo.lock"
        fi
        container -v "$dir:/repo" -w /repo "$image" cargo fetch --locked
    done
    # The image's toolchain, not whatever `stable` (rust-toolchain.toml) is
    # on the day.
    container -v "$root:/src:ro" -e CARGO_TARGET_DIR=/build -e RUSTUP_TOOLCHAIN=1.99.1 \
        -w /src "$image" cargo build --release --locked -p codeatlas-cli
}

probe() {
    local name="$1" set="$2" seed sample
    seed="$(field "r['probes']['$set']['seed']")"
    sample="$(field "r['probes']['$set']['sample']")"
    mkdir -p "$here/raw"
    # Network off: the probes build and run the repository's own code.
    container --network none -e CARGO_NET_OFFLINE=true \
        -v "$work/repos/$name:/repo:ro" -v "$here/raw:/out" "$image" \
        /build/release/codeatlas probe-tests /repo -n "$sample" --seed "$seed" \
        -o "/out/$name-$set-truth.json"
}

compare() {
    local name="$1" set="$2"
    mkdir -p "$here/results"
    cargo run --quiet --release --manifest-path "$root/Cargo.toml" -p codeatlas-cli -- \
        compare-tests "$work/repos/$name" --truth "$here/raw/$name-$set-truth.json" \
        -o "$here/results/$name-$set-comparison.json"
}

case "${1:-}" in
    setup) setup ;;
    probe) probe "$2" "$3" ;;
    compare) compare "$2" "$3" ;;
    *) sed -n '2,12p' "$0"; exit 2 ;;
esac
