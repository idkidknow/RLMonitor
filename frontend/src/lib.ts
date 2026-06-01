export type EventKind = 'chat' | 'connected' | 'disconnected';
export type FilterKind = 'all' | 'chat' | 'connections';
export interface ChatEvent {
  id: string;
  timestamp: string;
  event_type: EventKind;
  content: string | null;
  raw_json: string | null;
}
export interface HistoryPage {
  events: ChatEvent[];
  has_more: boolean;
}
export interface Overview {
  connected: boolean;
  server_name: string;
  retention_days: number;
  realitylink_address: string;
  stats: {
    stored_messages: number;
    messages_24h: number;
    last_message_at: string | null;
    activity: { hour: string; count: number }[];
  };
}

export interface ServerPlayer {
  uuid: string;
  name: string;
  online: boolean;
}
export interface PlayerStatistics extends ServerPlayer {
  play_time_ticks: number | null;
  deaths: number | null;
  updated_at: string | null;
  error: boolean;
}
export interface PlayerSnapshot<T = ServerPlayer> {
  players: T[];
  updated_at: string | null;
  error: string | null;
  refreshing: boolean;
  revision: number;
}
export type RankingSnapshot = PlayerSnapshot<PlayerStatistics>;

const countValue = (value: unknown) => Number.isSafeInteger(value) && (value as number) >= 0;
const nullableDate = (value: unknown) =>
  value === null || (typeof value === 'string' && Number.isFinite(Date.parse(value)));
function isServerPlayer(value: unknown): value is ServerPlayer {
  if (!value || typeof value !== 'object') return false;
  const p = value as Record<string, unknown>;
  return typeof p.uuid === 'string' && typeof p.name === 'string' && typeof p.online === 'boolean';
}
export function isPlayerSnapshot(
  value: unknown,
  statistics = false,
): value is PlayerSnapshot | RankingSnapshot {
  if (!value || typeof value !== 'object') return false;
  const snapshot = value as Record<string, unknown>;
  return (
    Array.isArray(snapshot.players) &&
    nullableDate(snapshot.updated_at) &&
    (snapshot.error === null || typeof snapshot.error === 'string') &&
    typeof snapshot.refreshing === 'boolean' &&
    countValue(snapshot.revision) &&
    snapshot.players.every((p) => {
      if (!isServerPlayer(p)) return false;
      if (!statistics) return true;
      const row = p as PlayerStatistics;
      return (
        (row.play_time_ticks === null || countValue(row.play_time_ticks)) &&
        (row.deaths === null || countValue(row.deaths)) &&
        nullableDate(row.updated_at) &&
        typeof row.error === 'boolean'
      );
    })
  );
}
export async function fetchPlayerSnapshot<T extends ServerPlayer>(
  resource: 'players' | 'rankings',
  refresh = false,
): Promise<PlayerSnapshot<T>> {
  const response = await fetch(`/api/${resource}${refresh ? '/refresh' : ''}`, {
    method: refresh ? 'POST' : 'GET',
    signal: AbortSignal.timeout(refresh ? 150_000 : 10_000),
  });
  if (!response.ok) throw new Error('无法读取玩家数据，请重试');
  const value: unknown = await response.json();
  if (!isPlayerSnapshot(value, resource === 'rankings'))
    throw new Error('服务器返回了无法识别的玩家数据');
  return value as PlayerSnapshot<T>;
}
export function rankPlayers(players: PlayerStatistics[], metric: 'play_time_ticks' | 'deaths') {
  const sorted = players
    .filter((p) => p[metric] !== null)
    .toSorted(
      (a, b) =>
        b[metric]! - a[metric]! || a.name.localeCompare(b.name) || a.uuid.localeCompare(b.uuid),
    );
  let rank = 0;
  return sorted.map((player, index) => {
    if (index === 0 || player[metric] !== sorted[index - 1][metric]) rank = index + 1;
    return { player, value: player[metric]!, rank };
  });
}
export function playTimeLabel(ticks: number): string {
  const seconds = Math.floor(ticks / 20);
  if (seconds < 60) return `${seconds} 秒`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes} 分钟`;
  const hours = Math.floor(minutes / 60);
  return `${hours.toLocaleString('zh-CN')} 小时${minutes % 60 ? ` ${minutes % 60} 分钟` : ''}`;
}

export const stripFormatting = (text: string) => text.replace(/§[0-9a-fk-or]/gi, '');

export function componentText(value: unknown, depth = 0): string {
  if (depth > 32) return '';
  if (typeof value === 'string' || typeof value === 'number') return String(value);
  if (Array.isArray(value)) return value.map((v) => componentText(v, depth + 1)).join('');
  if (!value || typeof value !== 'object') return '';
  const object = value as Record<string, unknown>;
  const base =
    typeof object.text === 'string'
      ? object.text
      : Array.isArray(object.with)
        ? object.with.map((v) => componentText(v, depth + 1)).join(' ')
        : '';
  return (
    base +
    (Array.isArray(object.extra)
      ? object.extra.map((v) => componentText(v, depth + 1)).join('')
      : '')
  );
}

export type MessageKind = 'chat' | 'joined' | 'left' | 'advancement' | 'death' | 'system';
export interface MessageView {
  player: string | null;
  players: string[];
  text: string;
  kind: MessageKind;
  label: string;
}
const messageLabels: Record<MessageKind, string> = {
  chat: '玩家聊天',
  joined: '加入游戏',
  left: '退出游戏',
  advancement: '进度',
  death: '死亡',
  system: '系统消息',
};

function relatedPlayers(value: unknown, depth = 0): string[] {
  if (depth > 32 || !value || typeof value !== 'object') return [];
  if (Array.isArray(value)) return value.flatMap((v) => relatedPlayers(v, depth + 1));
  const component = value as Record<string, unknown>;
  const players: string[] = [];
  const hover = component.hoverEvent;
  if (hover && typeof hover === 'object' && !Array.isArray(hover)) {
    const contents = (hover as Record<string, unknown>).contents;
    if (contents && typeof contents === 'object' && !Array.isArray(contents)) {
      const entity = contents as Record<string, unknown>;
      if (entity.type === 'minecraft:player') {
        const name = stripFormatting(
          componentText(entity.name) ||
            componentText(component) ||
            (typeof component.insertion === 'string' ? component.insertion : ''),
        ).trim();
        if (name) players.push(name);
      }
    }
  }
  return players.concat(
    relatedPlayers(component.with, depth + 1),
    relatedPlayers(component.extra, depth + 1),
  );
}

export function messageView(event: ChatEvent): MessageView {
  let raw: unknown;
  try {
    raw = JSON.parse(event.raw_json ?? 'null');
  } catch {
    raw = null;
  }
  let text = stripFormatting(event.content ?? componentText(raw));
  // When translation is unavailable, the backend may return the raw component string.
  if (text === event.raw_json) text = stripFormatting(componentText(raw) || text);
  let kind: MessageKind = 'system';
  let player: string | null = null;
  const players = new Set(relatedPlayers(raw));
  if (raw && typeof raw === 'object' && !Array.isArray(raw)) {
    const object = raw as Record<string, unknown>;
    const key = typeof object.translate === 'string' ? object.translate : '';
    if (key === 'multiplayer.player.joined') kind = 'joined';
    else if (key === 'multiplayer.player.left') kind = 'left';
    else if (key.startsWith('chat.type.advancement.')) kind = 'advancement';
    else if (key.startsWith('death.')) kind = 'death';
    if (object.translate === 'chat.type.text' && Array.isArray(object.with)) {
      kind = 'chat';
      player = stripFormatting(componentText(object.with[0])).trim() || null;
    }
  }
  const match = text.match(/^<([^>]{1,48})>\s*([\s\S]*)$/);
  if (match && (kind === 'chat' || kind === 'system')) {
    kind = 'chat';
    player = match[1];
    text = match[2];
  }
  player ??= players.values().next().value ?? null;
  if (player) players.add(player);
  return { player, players: [...players], text, kind, label: messageLabels[kind] };
}

export function matches(event: ChatEvent, kind: FilterKind, query: string): boolean {
  if (kind === 'chat' && event.event_type !== 'chat') return false;
  if (kind === 'connections' && event.event_type === 'chat') return false;
  return !query || (event.content ?? '').toLowerCase().includes(query.toLowerCase());
}

export function mergeEvents(
  current: ChatEvent[],
  incoming: ChatEvent[],
  prepend = false,
): ChatEvent[] {
  const seen = new Set<string>();
  return (prepend ? [...incoming, ...current] : [...current, ...incoming]).filter((event) => {
    if (seen.has(event.id)) return false;
    seen.add(event.id);
    return true;
  });
}

export function isChatEvent(value: unknown): value is ChatEvent {
  if (!value || typeof value !== 'object') return false;
  const event = value as Record<string, unknown>;
  return (
    typeof event.id === 'string' &&
    typeof event.timestamp === 'string' &&
    ['chat', 'connected', 'disconnected'].includes(String(event.event_type)) &&
    (event.content === null || typeof event.content === 'string') &&
    (event.raw_json === null || typeof event.raw_json === 'string')
  );
}

export class ApiError extends Error {
  status: number;
  constructor(status: number) {
    super(status === 410 ? '记录已过期，请刷新时间线' : '暂时无法读取记录，请重试');
    this.status = status;
  }
}

export async function fetchHistory(
  params: URLSearchParams,
  signal?: AbortSignal,
): Promise<HistoryPage> {
  const response = await fetch(`/api/history?${params}`, { signal });
  if (!response.ok) throw new ApiError(response.status);
  const page = (await response.json()) as HistoryPage;
  if (
    !Array.isArray(page.events) ||
    !page.events.every(isChatEvent) ||
    typeof page.has_more !== 'boolean'
  )
    throw new Error('服务器返回了无法识别的记录');
  return page;
}

export const localDay = (timestamp: string) => new Date(timestamp).toLocaleDateString('sv-SE');
export const timeLabel = (timestamp: string) =>
  new Date(timestamp).toLocaleTimeString('zh-CN', {
    hour12: false,
    hour: '2-digit',
    minute: '2-digit',
  });
export const fullDate = (timestamp: string) =>
  new Date(timestamp).toLocaleString('zh-CN', { hour12: false });
export function dayLabel(timestamp: string): string {
  const day = localDay(timestamp);
  if (day === localDay(new Date().toISOString())) return '今天';
  if (day === localDay(new Date(Date.now() - 86_400_000).toISOString())) return '昨天';
  return new Date(timestamp).toLocaleDateString('zh-CN', {
    month: 'long',
    day: 'numeric',
    weekday: 'short',
  });
}

export function prettyJson(raw: string | null): string {
  if (!raw) return '此事件没有原始消息';
  try {
    return JSON.stringify(JSON.parse(raw), null, 2);
  } catch {
    return raw;
  }
}

export function exportEvents(events: ChatEvent[]): void {
  const blob = new Blob([JSON.stringify(events, null, 2)], {
    type: 'application/json;charset=utf-8',
  });
  const url = URL.createObjectURL(blob);
  const link = document.createElement('a');
  link.href = url;
  link.download = `RLMonitor-${localDay(new Date().toISOString())}.json`;
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
