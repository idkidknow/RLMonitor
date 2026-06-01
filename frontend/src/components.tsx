import { For } from 'solid-js';

export function WorldMark() {
  return (
    <svg viewBox="0 0 40 40" fill="none" aria-hidden="true">
      <path d="m20 4 16 9-16 9-16-9Z" fill="currentColor" opacity=".95" />
      <path d="m4 13 16 9v15L4 28Z" fill="currentColor" opacity=".5" />
      <path d="m20 22 16-9v15l-16 9Z" fill="currentColor" opacity=".72" />
      <path d="m12 9 16 9M20 4v18" stroke="#173e32" opacity=".2" />
    </svg>
  );
}

export function Avatar(props: { name: string }) {
  const hash = () => [...props.name].reduce((n, char) => (n * 31 + char.charCodeAt(0)) >>> 0, 7);
  const palette = () => ['#558575', '#ad8059', '#7b779d', '#799752', '#5e87a3'][hash() % 5];
  const pixels = () =>
    Array.from({ length: 25 }, (_, i) => {
      const x = i % 5,
        y = Math.floor(i / 5);
      return { x, y, on: ((hash() >>> ((y * 3 + Math.min(x, 4 - x)) % 24)) & 1) === 1 };
    });
  return (
    <span class="avatar" style={{ '--avatar-color': palette() }} aria-hidden="true">
      <svg viewBox="0 0 7 7" shape-rendering="crispEdges">
        <For each={pixels()}>
          {(p) => p.on && <rect x={p.x + 1} y={p.y + 1} width="1" height="1" fill={palette()} />}
        </For>
      </svg>
    </span>
  );
}
