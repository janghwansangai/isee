#!/bin/bash
# 비공개 저장소(isee)의 릴리스 초안을 공개 배포 저장소(isee-releases)로 옮긴다.
# 앱의 자동 업데이트는 isee-releases의 latest.json을 본다.
#   scripts/publish.sh v0.1.0
set -euo pipefail
TAG=${1:?"태그를 적어 주세요 (예: v0.1.0)"}
SRC=janghwansangai/isee
DST=janghwansangai/isee-releases
W=$(mktemp -d)
trap 'rm -rf "$W"' EXIT
gh release download "$TAG" -R "$SRC" -D "$W"
# 업데이트 파일 주소를 공개 저장소로 바꾼다
sed -i '' "s#github.com/$SRC/releases/download#github.com/$DST/releases/download#g" "$W/latest.json"
NOTES=$(gh release view "$TAG" -R "$SRC" --json body -q .body)
gh release create "$TAG" -R "$DST" --title "iSee $TAG" --notes "$NOTES" "$W"/*
echo "공개했습니다: https://github.com/$DST/releases/tag/$TAG"
