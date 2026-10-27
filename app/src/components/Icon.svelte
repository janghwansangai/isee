<script lang="ts">
  let { name, size = 16 }: { name: string; size?: number } = $props();

  // 24×24 격자, 선 아이콘
  const paths: Record<string, string> = {
    dir: "M3 7.5A1.5 1.5 0 0 1 4.5 6h4.4l1.8 2h8.8A1.5 1.5 0 0 1 21 9.5v8A1.5 1.5 0 0 1 19.5 19h-15A1.5 1.5 0 0 1 3 17.5z",
    image: "M4 5h16v14H4z M4 16l4.5-4.5 3.5 3.5 2.5-2.5L20 17 M15.5 9.5a1.5 1.5 0 1 0 0 .01",
    video: "M4 6h12v12H4z M16 10l4-2.5v9L16 14",
    pdf: "M6 3h8l4 4v14H6z M14 3v4h4 M8.5 15.5h7 M8.5 12h7",
    doc: "M6 3h8l4 4v14H6z M14 3v4h4 M8.5 11h7 M8.5 14h7 M8.5 17h5",
    other: "M6 3h8l4 4v14H6z M14 3v4h4",
    home: "M4 11l8-6.5 8 6.5 M6 9.5V19h4.5v-5h3v5H18V9.5",
    desktop: "M3.5 5h17v11h-17z M9 20h6 M12 16v4",
    pictures: "M4 5h16v14H4z M4 16l4.5-4.5 3.5 3.5 2.5-2.5L20 17 M15.5 9.5a1.5 1.5 0 1 0 0 .01",
    downloads: "M12 4v11 M7.5 10.5 12 15l4.5-4.5 M5 19.5h14",
    documents: "M6 3h8l4 4v14H6z M14 3v4h4",
    volume: "M3.5 13.5h17v5h-17z M5 13.5l2-7.5h10l2 7.5 M17 16h.01",
    cloud: "M7 18.5a4 4 0 0 1-.6-7.95A5.5 5.5 0 0 1 17 9a3.75 3.75 0 0 1 .5 7.5z",
    back: "M15 5l-7 7 7 7",
    forward: "M9 5l7 7-7 7",
    up: "M5 15l7-7 7 7",
    chevron: "M9 6l6 6-6 6",
    info: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18z M12 11v5.5 M12 7.6v.01",
    close: "M6 6l12 12 M18 6L6 18",
    trash: "M4.5 7h15 M9.5 7V4.5h5V7 M6.5 7l1 13h9l1-13",
    bolt: "M13 3L5 13.5h6L10 21l8-10.5h-6z",
    sort: "M7 5v14 M4 16l3 3 3-3 M17 19V5 M14 8l3-3 3 3",
    grid: "M4.5 4.5h6v6h-6z M13.5 4.5h6v6h-6z M4.5 13.5h6v6h-6z M13.5 13.5h6v6h-6z",
    list: "M4 6.5h.01 M8 6.5h12 M4 12h.01 M8 12h12 M4 17.5h.01 M8 17.5h12",
  };
  let d = $derived(paths[name] ?? paths.other);
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width="1.6"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
>
  <path {d} />
</svg>
