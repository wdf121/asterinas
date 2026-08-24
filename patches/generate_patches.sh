#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'USAGE'
Usage: patches/generate_patches.sh [--base <ref>] [--output <dir>] [--no-untracked]

Regenerate one patch per changed file relative to <ref>.

Defaults:
  --base main
  --output <script directory>

Notes:
  - Existing numbered patches in the output directory are removed first.
  - The output directory itself is excluded, so patch files and these scripts are not patched.
  - Tracked additions/modifications/deletions are generated with git diff --binary <base>.
  - Untracked files are included by default using git diff --no-index --binary.
USAGE
}

base_ref="main"
output_dir=""
include_untracked=1

while (($#)); do
    case "$1" in
        --base)
            [[ $# -ge 2 ]] || { echo "missing value for --base" >&2; exit 2; }
            base_ref="$2"
            shift 2
            ;;
        --output)
            [[ $# -ge 2 ]] || { echo "missing value for --output" >&2; exit 2; }
            output_dir="$2"
            shift 2
            ;;
        --no-untracked)
            include_untracked=0
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

repo_root=$(git rev-parse --show-toplevel)
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)

if [[ -z "$output_dir" ]]; then
    output_dir="$script_dir"
fi
mkdir -p -- "$output_dir"
output_dir=$(cd -- "$output_dir" && pwd)

cd "$repo_root"

git rev-parse --verify --quiet "$base_ref^{commit}" >/dev/null || {
    echo "base ref not found: $base_ref" >&2
    exit 2
}

if [[ "$output_dir" == "$repo_root" ]]; then
    echo "output dir must not be repository root" >&2
    exit 2
fi

case "$output_dir" in
    "$repo_root"/*) ;;
    *)
        echo "output dir must be inside repository: $repo_root" >&2
        exit 2
        ;;
esac

output_rel=$(realpath --relative-to="$repo_root" "$output_dir")

filter_output_dir() {
    local path
    while IFS= read -r path; do
        if [[ "$path" != "$output_rel" && "$path" != "$output_rel/"* ]]; then
            printf '%s\n' "$path"
        fi
    done
}

# Collect tracked changes relative to the base ref, including working-tree edits.
mapfile -t tracked_files < <(
    git diff --name-only --diff-filter=ACMDRT "$base_ref" -- . \
        | filter_output_dir \
        | LC_ALL=C sort -u
)

untracked_files=()
if ((include_untracked)); then
    mapfile -t untracked_files < <(
        git ls-files --others --exclude-standard \
            | filter_output_dir \
            | LC_ALL=C sort -u
    )
fi

mapfile -t files < <(
    printf '%s\n' "${tracked_files[@]}" "${untracked_files[@]}" \
        | sed '/^$/d' \
        | LC_ALL=C sort -u
)

find "$output_dir" -maxdepth 1 -type f -name '[0-9][0-9][0-9]-*.patch' -delete

sanitize_path() {
    local path="$1"
    path=${path#./}
    path=${path//\//__}
    path=${path// /_}
    path=${path//[^A-Za-z0-9._-]/_}
    while [[ "$path" == .* ]]; do
        path=${path#.}
    done
    [[ -n "$path" ]] || path="root"
    printf '%s' "$path"
}

count=0
for file in "${files[@]}"; do
    ((count += 1))
    patch_name=$(printf '%03d-%s.patch' "$count" "$(sanitize_path "$file")")
    patch_path="$output_dir/$patch_name"

    if git ls-files --error-unmatch -- "$file" >/dev/null 2>&1; then
        git diff --binary "$base_ref" -- "$file" > "$patch_path"
    else
        git diff --binary --no-index -- /dev/null "$file" > "$patch_path" || true
    fi

    if [[ ! -s "$patch_path" ]]; then
        rm -f -- "$patch_path"
        ((count -= 1))
    fi
done

empty_count=$(find "$output_dir" -maxdepth 1 -type f -name '[0-9][0-9][0-9]-*.patch' -empty | wc -l)
patch_count=$(find "$output_dir" -maxdepth 1 -type f -name '[0-9][0-9][0-9]-*.patch' | wc -l)

echo "base_ref=$base_ref"
echo "output_dir=$output_dir"
echo "patch_count=$patch_count"
echo "empty_count=$empty_count"

if [[ "$empty_count" != "0" ]]; then
    echo "error: generated empty patches" >&2
    exit 1
fi
