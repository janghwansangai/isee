#!/usr/bin/env python3
"""iSee 오프라인 지명 데이터(places.bin) 만들기.

    python3 -I scripts/build_places.py <cities5000.txt> <HangJeongDong.geojson> crates/isee-core/data/places.bin

입력 데이터 (출처 표시 필요, README의 '데이터 출처' 참고)
  - 해외 도시: GeoNames cities5000 (https://download.geonames.org/export/dump/, CC BY 4.0)
  - 국내 행정동 경계: admdongkor (https://github.com/vuski/admdongkor)
나라 이름 한글은 macOS의 한국어 지역 이름표에서 가져온다 (맥에서만 실행).

형식 (리틀 엔디언, 정수는 LEB128 가변 길이, 좌표는 지그재그 부호)
  "ISPL" u16 버전
  문자열 표: 개수, (길이, UTF-8)…
  나라: 개수, (코드 2바이트, 이름)…
  도시: 개수, (이름, 나라, 위도×1e4, 경도×1e4, 인구÷1000)…
  시도: 개수, (이름, 짧은 이름)…
  시군구: 개수, (시도, 이름)…
  읍면동: 개수, (시군구, 이름, 고리 수, (점 수, 첫 점 ×1e5, 이후 차이…)…)…
"""
import json
import re
import struct
import subprocess
import sys

HANGUL = re.compile("[가-힣]")


def varint(n: int) -> bytes:
    out = bytearray()
    while True:
        b = n & 0x7F
        n >>= 7
        if n:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)


def zz(n: int) -> bytes:
    return varint((n << 1) ^ (n >> 63) if n < 0 else n << 1)


class Strings:
    def __init__(self):
        self.idx = {}
        self.items = []

    def __call__(self, s: str) -> int:
        if s not in self.idx:
            self.idx[s] = len(self.items)
            self.items.append(s)
        return self.idx[s]


def korean_country_names(codes):
    js = (
        'ObjC.import("Foundation"); var l=$.NSLocale.alloc.initWithLocaleIdentifier("ko");'
        + "JSON.stringify(%s.map(c=>[c, ObjC.unwrap(l.displayNameForKeyValue($.NSLocaleCountryCode, c))]))"
        % json.dumps(sorted(codes))
    )
    out = subprocess.run(["osascript", "-l", "JavaScript", "-e", js], capture_output=True, text=True, check=True)
    return {c: (n or c) for c, n in json.loads(out.stdout)}


def city_name(cols):
    for n in cols[3].split(","):
        n = n.strip()
        if HANGUL.search(n) and not re.search("[A-Za-z]", n):
            # "오사카 시" → "오사카"
            return re.sub(r"\s+시$", "", n)
    return cols[2] or cols[1]


def simplify(pts, tol):
    """더글러스-포커 단순화 (닫힌 고리: 첫 점 = 끝 점)"""
    if len(pts) <= 4:
        return pts
    keep = [False] * len(pts)
    keep[0] = keep[-1] = True
    stack = [(0, len(pts) - 1)]
    while stack:
        a, b = stack.pop()
        (x1, y1), (x2, y2) = pts[a], pts[b]
        dx, dy = x2 - x1, y2 - y1
        norm = (dx * dx + dy * dy) ** 0.5
        best, bi = -1.0, -1
        for i in range(a + 1, b):
            x, y = pts[i]
            d = abs(dy * (x - x1) - dx * (y - y1)) / norm if norm else ((x - x1) ** 2 + (y - y1) ** 2) ** 0.5
            if d > best:
                best, bi = d, i
        if bi >= 0 and best > tol:
            keep[bi] = True
            stack += [(a, bi), (bi, b)]
    out = [p for p, k in zip(pts, keep) if k]
    # 고리가 너무 작아져도 삼각형은 남긴다
    return out if len(out) >= 4 else pts[:: max(1, len(pts) // 3)] + [pts[-1]]


SIDO_SHORT = {
    "충청북도": "충북",
    "충청남도": "충남",
    "전라북도": "전북",
    "전라남도": "전남",
    "경상북도": "경북",
    "경상남도": "경남",
}


def sido_short(name):
    if name in SIDO_SHORT:
        return SIDO_SHORT[name]
    for suffix in ["통합특별시", "특별자치시", "특별자치도", "특별시", "광역시", "도"]:
        if name.endswith(suffix) and len(name) > len(suffix):
            short = name[: -len(suffix)]
            return SIDO_SHORT.get(short + "도", short)
    return name


def sgg_name(name):
    # "수원시장안구" → "수원시 장안구"
    m = re.match(r"^(.+시)(.+구)$", name)
    return f"{m.group(1)} {m.group(2)}" if m else name


def main(cities_path, geojson_path, out_path):
    strings = Strings()
    cities = []
    codes = set()
    with open(cities_path, encoding="utf-8") as f:
        for line in f:
            c = line.rstrip("\n").split("\t")
            # 도시 안의 구역(PPLX)·옛 지명·버려진 곳은 뺀다 ("Paris 16 Passy" 대신 "파리")
            if len(c) < 15 or not c[8] or c[7] in ("PPLX", "PPLH", "PPLQ", "PPLW", "PPLCH"):
                continue
            codes.add(c[8])
            cities.append((city_name(c), c[8], float(c[4]), float(c[5]), int(c[14] or 0)))
    countries = korean_country_names(codes)
    country_list = sorted(countries)
    country_idx = {c: i for i, c in enumerate(country_list)}

    geo = json.load(open(geojson_path, encoding="utf-8"))
    sidos, sggs, dongs = [], [], []
    sido_idx, sgg_idx = {}, {}
    total_in = total_out = 0
    for feat in geo["features"]:
        p = feat["properties"]
        sido = p["sidonm"]
        if sido not in sido_idx:
            sido_idx[sido] = len(sidos)
            sidos.append(sido)
        # 세종은 시군구가 없다 ("세종시")
        sgg = p["sggnm"] or ""
        key = (sido, sgg)
        if key not in sgg_idx:
            sgg_idx[key] = len(sggs)
            sggs.append((sido_idx[sido], sgg_name(sgg) if not sido.startswith("세종") else ""))
        dong = p["adm_nm"].split(" ")[-1]
        rings = []
        g = feat["geometry"]
        polys = g["coordinates"] if g["type"] == "MultiPolygon" else [g["coordinates"]]
        for poly in polys:
            for ring in poly:
                total_in += len(ring)
                s = simplify(ring, 0.0002)
                total_out += len(s)
                rings.append(s)
        dongs.append((sgg_idx[key], dong, rings))

    out = bytearray(b"ISPL" + struct.pack("<H", 1))
    body = bytearray()
    body += varint(len(country_list))
    for c in country_list:
        body += c.encode("ascii") + varint(strings(countries[c]))
    body += varint(len(cities))
    for name, cc, lat, lon, pop in cities:
        body += varint(strings(name)) + varint(country_idx[cc]) + zz(round(lat * 1e4)) + zz(round(lon * 1e4))
        body += varint(pop // 1000)
    body += varint(len(sidos))
    for s in sidos:
        body += varint(strings(s)) + varint(strings(sido_short(s)))
    body += varint(len(sggs))
    for si, name in sggs:
        body += varint(si) + varint(strings(name))
    body += varint(len(dongs))
    for gi, name, rings in dongs:
        body += varint(gi) + varint(strings(name)) + varint(len(rings))
        for ring in rings:
            body += varint(len(ring))
            px = py = 0
            for x, y in ring:
                ix, iy = round(x * 1e5), round(y * 1e5)
                body += zz(ix - px) + zz(iy - py)
                px, py = ix, iy
    out += varint(len(strings.items))
    for s in strings.items:
        b = s.encode("utf-8")
        out += varint(len(b)) + b
    out += body
    with open(out_path, "wb") as f:
        f.write(out)
    print(
        f"나라 {len(country_list)} · 도시 {len(cities)} · 시도 {len(sidos)} · 시군구 {len(sggs)} · 읍면동 {len(dongs)}"
        f" · 경계 점 {total_in:,} → {total_out:,} · {len(out) / 1e6:.2f}MB"
    )


if __name__ == "__main__":
    main(*sys.argv[1:4])
