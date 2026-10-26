#!/bin/bash
# 앱 실행 → 첫 화면(폴더 목록이 그려질 때)까지의 시간을 바깥에서 잰다.
# 사용법: scripts/measure_launch.sh [횟수]
APP="$(dirname "$0")/../target/release/bundle/macos/iSee.app/Contents/MacOS/isee-app"
N=${1:-5}
LOG=$(mktemp)
now_ms() { python3 -c 'import time; print(int(time.time()*1000))'; }
for i in $(seq 1 "$N"); do
  : > "$LOG"
  start=$(now_ms)
  "$APP" 2> "$LOG" &
  pid=$!
  for _ in $(seq 1 1000); do
    grep -q "첫 화면까지" "$LOG" && break
    sleep 0.005
  done
  end=$(now_ms)
  inner=$(grep -o "첫 화면까지 [0-9]*ms" "$LOG")
  echo "#$i 바깥에서 잰 시간 $((end - start))ms (앱 내부 기록: $inner)"
  kill "$pid" 2>/dev/null
  wait "$pid" 2>/dev/null
  sleep 0.5
done
rm -f "$LOG"
