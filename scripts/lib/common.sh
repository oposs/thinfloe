#!/bin/bash
# common.sh - Shared build utilities

set -euo pipefail

# Color codes
readonly RED='\033[0;31m'
readonly GREEN='\033[0;32m'
readonly BLUE='\033[0;34m'
readonly CYAN='\033[0;36m'
readonly NC='\033[0m'

# Logging functions
log_info() {
    echo -e "${BLUE}[INFO]${NC} $*" >&2
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $*" >&2
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $*" >&2
}

log_step() {
    echo -e "${CYAN}==>${NC} $*" >&2
}

die() {
    log_error "$*"
    exit 1
}

# Verify file exists and has minimum size
verify_file() {
    local file="$1"
    local min_size="${2:-1}"

    [[ -f "$file" ]] || die "File not found: $file"

    local size=$(stat -c%s "$file" 2>/dev/null || stat -f%z "$file")
    [[ "$size" -ge "$min_size" ]] || \
        die "File $file is too small (${size} bytes, expected >= ${min_size})"

    log_info "Verified: $file (${size} bytes)"
}

# Create directory if it doesn't exist
ensure_dir() {
    local dir="$1"
    [[ -d "$dir" ]] || mkdir -p "$dir"
}

# Format bytes to human readable. MB and GB carry one decimal, rounded:
# integer division printed the 2.93 GiB USB image as "2GB".
format_bytes() {
    local bytes=$1 unit tenths
    if [ $bytes -lt 1024 ]; then
        echo "${bytes}B"
        return
    elif [ $bytes -lt 1048576 ]; then
        echo "$((bytes / 1024))KB"
        return
    elif [ $bytes -lt 1073741824 ]; then
        unit=MB
        tenths=$(((bytes * 10 + 524288) / 1048576))
    else
        unit=GB
        tenths=$(((bytes * 10 + 536870912) / 1073741824))
    fi
    echo "$((tenths / 10)).$((tenths % 10))${unit}"
}
