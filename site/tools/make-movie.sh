#!/bin/bash
# Cut the boot movie from recorded frames (qmp.py record, 15 frames a second).
#
#   make-movie.sh FRAMEDIR WORKDIR OUTDIR RANGE...
#
# A RANGE is FIRST-LAST (frame numbers, both included). The ranges are joined
# and every second frame is kept at 30 frames a second, so the movie runs
# four times as fast as the boot did. Writes OUTDIR/boot.webm and
# OUTDIR/boot.mp4, without sound. Uses 4 threads.
set -euo pipefail
frames=$(realpath "$1"); work=$(realpath "$2"); out=$(realpath "$3"); shift 3
list="$work/movie-frames.txt"
: > "$list"
for range in "$@"; do
    for ((n = ${range%-*}; n <= ${range#*-}; n += 2)); do
        printf "file '%s/f%06d.ppm'\nduration 0.0333333\n" "$frames" "$n" >> "$list"
    done
done
in=(-f concat -safe 0 -i "$list" -an -r 30 -pix_fmt yuv420p -threads 4)
ffmpeg -loglevel error -y "${in[@]}" -c:v libvpx-vp9 -b:v 0 -crf "${TF_WEBM_CRF:-38}" -row-mt 1 -g 300 "$out/boot.webm"
ffmpeg -loglevel error -y "${in[@]}" -c:v libx264 -preset slow -crf "${TF_MP4_CRF:-27}" -profile:v high -movflags +faststart "$out/boot.mp4"
ls -l "$out/boot.webm" "$out/boot.mp4"
