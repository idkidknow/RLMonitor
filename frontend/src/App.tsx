import {
  For,
  Show,
  batch,
  createEffect,
  createMemo,
  createSignal,
  on,
  onCleanup,
  onMount,
} from 'solid-js';
import {
  Activity,
  ArrowDown,
  ArrowDownToLine,
  ArrowLeft,
  ArrowUpRight,
  BookOpen,
  Check,
  ChevronDown,
  ChevronRight,
  CircleHelp,
  Copy,
  Database,
  FileJson,
  Hash,
  LoaderCircle,
  MessageSquare,
  RefreshCw,
  Search,
  Settings2,
  Sprout,
  Trophy,
  Users,
  Wifi,
  WifiOff,
  X,
} from 'lucide-solid';
import { Avatar, WorldMark } from './components';
import { PlayersPage, RankingsPage } from './Players';
import {
  ApiError,
  dayLabel,
  exportEvents,
  fetchHistory,
  fetchPlayerSnapshot,
  fullDate,
  isChatEvent,
  isPlayerSnapshot,
  localDay,
  matches,
  mergeEvents,
  messageView,
  prettyJson,
  timeLabel,
} from './lib';
import type {
  ChatEvent,
  FilterKind,
  Overview,
  PlayerSnapshot,
  RankingSnapshot,
  ServerPlayer,
  PlayerStatistics,
} from './lib';

const PAGE_SIZE = 100;
const number = (value: number) => value.toLocaleString('zh-CN');

export default function App() {
  const [overview, setOverview] = createSignal<Overview>();
  const [page, setPage] = createSignal<'chat' | 'players' | 'rankings'>('chat');
  const [players, setPlayers] = createSignal<PlayerSnapshot>();
  const [rankings, setRankings] = createSignal<RankingSnapshot>();
  const [playersError, setPlayersError] = createSignal('');
  const [rankingsError, setRankingsError] = createSignal('');
  const [refreshingPlayers, setRefreshingPlayers] = createSignal(false);
  const [refreshingRankings, setRefreshingRankings] = createSignal(false);
  const [connected, setConnected] = createSignal(false);
  const [streamState, setStreamState] = createSignal<'connecting' | 'open' | 'closed'>(
    'connecting',
  );
  const [events, setEvents] = createSignal<ChatEvent[]>([]);
  const [kind, setKind] = createSignal<FilterKind>('all');
  const [search, setSearch] = createSignal('');
  const [query, setQuery] = createSignal('');
  const [loading, setLoading] = createSignal(true);
  const [loadingOlder, setLoadingOlder] = createSignal(false);
  const [hasMore, setHasMore] = createSignal(false);
  const [error, setError] = createSignal('');
  const [following, setFollowing] = createSignal(true);
  const [unread, setUnread] = createSignal(0);
  const [selected, setSelected] = createSignal<ChatEvent>();
  const [toast, setToast] = createSignal('');
  const [settings, setSettings] = createSignal(false);
  let feed!: HTMLDivElement;
  let searchInput!: HTMLInputElement;
  let dialog!: HTMLDialogElement;
  let source: EventSource | undefined;
  let controller: AbortController | undefined;
  let generation = 0;
  let liveCursor: string | undefined;
  let buffered: ChatEvent[] = [];
  let recovering = false;
  let recoveryRequested = false;
  let needsRecovery = false;
  let recoveryTimer: ReturnType<typeof setTimeout> | undefined;
  let toastTimer: ReturnType<typeof setTimeout>;
  let statsTimer: ReturnType<typeof setTimeout> | undefined;
  let destroyed = false;
  let playerConnection = 0;
  let playersRevision = -1;
  let rankingsRevision = -1;

  const groups = createMemo(() => {
    const result: { day: string; label: string; items: ChatEvent[] }[] = [];
    for (const event of events()) {
      const day = localDay(event.timestamp);
      const last = result.at(-1);
      if (last?.day === day) last.items.push(event);
      else result.push({ day, label: dayLabel(event.timestamp), items: [event] });
    }
    return result;
  });
  const loadedSummary = createMemo(() => {
    const players = new Map<string, number>();
    let chatCount = 0;
    for (const event of events()) {
      if (event.event_type !== 'chat') continue;
      chatCount++;
      for (const name of messageView(event).players) {
        if (matches(event, 'chat', name)) players.set(name, (players.get(name) ?? 0) + 1);
      }
    }
    return {
      chatCount,
      connectionCount: events().length - chatCount,
      players: [...players]
        .map(([name, count]) => ({ name, count }))
        .sort((a, b) => b.count - a.count || a.name.localeCompare(b.name)),
    };
  });
  const stats = () => overview()?.stats;
  const pageTitle = () => ({ chat: '聊天记录', players: '玩家', rankings: '排行榜' })[page()];
  const onlineCount = () => players()?.players.filter((p) => p.online).length ?? 0;
  const activityMax = () => Math.max(1, ...(stats()?.activity.map((a) => a.count) ?? []));
  const statusText = () =>
    streamState() !== 'open'
      ? '正在连接记录服务'
      : connected()
        ? '正在接收游戏消息'
        : '等待服务器连接';

  function notify(message: string) {
    setToast(message);
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => setToast(''), 2800);
  }
  function toBottom() {
    requestAnimationFrame(() => {
      if (feed?.isConnected && page() === 'chat') feed.scrollTop = feed.scrollHeight;
    });
  }
  function follow() {
    setFollowing(true);
    setUnread(0);
    toBottom();
  }

  function acceptPlayers(value: PlayerSnapshot) {
    if (value.revision >= playersRevision) {
      playersRevision = value.revision;
      setPlayers(value);
    }
    setPlayersError('');
  }
  function acceptRankings(value: RankingSnapshot) {
    if (value.revision >= rankingsRevision) {
      rankingsRevision = value.revision;
      setRankings(value);
    }
    setRankingsError('');
  }
  async function readPlayerData(resource: 'players' | 'rankings', refresh = false) {
    const busy = resource === 'players' ? refreshingPlayers : refreshingRankings;
    const setBusy = resource === 'players' ? setRefreshingPlayers : setRefreshingRankings;
    if (refresh && busy()) return;
    if (refresh) setBusy(true);
    const connection = playerConnection;
    try {
      if (resource === 'players') {
        const value = await fetchPlayerSnapshot<ServerPlayer>('players', refresh);
        if (!destroyed && connection === playerConnection) acceptPlayers(value);
      } else {
        const value = await fetchPlayerSnapshot<PlayerStatistics>('rankings', refresh);
        if (!destroyed && connection === playerConnection) acceptRankings(value);
      }
    } catch (err) {
      if (!destroyed && connection === playerConnection)
        (resource === 'players' ? setPlayersError : setRankingsError)(
          err instanceof Error ? err.message : '无法读取玩家数据，请重试',
        );
    } finally {
      if (!destroyed && refresh) setBusy(false);
    }
  }
  function readPlayerSnapshots() {
    void readPlayerData('players');
    void readPlayerData('rankings');
  }
  function searchPlayer(name: string) {
    batch(() => {
      setPage('chat');
      setKind('chat');
      setSearch(name);
      setQuery(name);
      setSelected(undefined);
    });
  }

  async function refreshOverview() {
    try {
      const response = await fetch('/api/overview');
      if (!response.ok) throw new Error('overview unavailable');
      const value = (await response.json()) as Overview;
      if (!destroyed) {
        setOverview(value);
        setConnected(value.connected);
      }
    } catch {
      /* The timeline owns the actionable network error UI. */
    }
  }
  function scheduleOverview() {
    if (statsTimer) return;
    statsTimer = setTimeout(() => {
      statsTimer = undefined;
      void refreshOverview();
    }, 700);
  }
  function appendLive(event: ChatEvent) {
    if (event.id === liveCursor) return;
    liveCursor = event.id;
    if (event.event_type === 'connected') setConnected(true);
    if (event.event_type === 'disconnected') setConnected(false);
    if (matches(event, kind(), query())) {
      setEvents((current) => {
        const next = mergeEvents(current, [event]);
        if (following() && next.length > 2000) {
          setHasMore(true);
          return next.slice(-2000);
        }
        return next;
      });
      if (following()) toBottom();
      else setUnread((n) => n + 1);
    }
    scheduleOverview();
  }

  async function resetHistory() {
    const version = ++generation;
    if (recovering) recoveryRequested = true;
    controller?.abort();
    controller = new AbortController();
    const signal = controller.signal;
    setLoading(true);
    setError('');
    setSelected(undefined);
    setUnread(0);
    buffered = [];
    const params = new URLSearchParams({ limit: String(PAGE_SIZE), kind: kind(), q: query() });
    try {
      const [page, newest] = await Promise.all([
        fetchHistory(params, signal),
        fetchHistory(new URLSearchParams({ limit: '1' }), signal),
      ]);
      if (version !== generation || destroyed) return;
      setEvents(page.events);
      setHasMore(page.has_more);
      needsRecovery = false;
      clearTimeout(recoveryTimer);
      liveCursor = newest.events.at(-1)?.id;
      const snapshot = buffered.findIndex((e) => e.id === liveCursor);
      const pending = snapshot >= 0 ? buffered.slice(snapshot + 1) : buffered;
      buffered = [];
      setLoading(false);
      for (const event of pending) appendLive(event);
      if (following()) toBottom();
      if (recoveryRequested) {
        recoveryRequested = false;
        void recover();
      }
    } catch (err) {
      if (version !== generation || signal.aborted || destroyed) return;
      setError(err instanceof Error ? err.message : '无法读取记录');
      setLoading(false);
    }
  }

  async function recover() {
    if (loading() || recovering) {
      recoveryRequested = true;
      return;
    }
    if (!liveCursor) {
      await resetHistory();
      return;
    }
    recovering = true;
    const version = generation;
    const received = new Set<string>();
    let cursor = liveCursor;
    let succeeded = false;
    try {
      while (!destroyed && version === generation) {
        const page = await fetchHistory(
          new URLSearchParams({ after: cursor, limit: String(PAGE_SIZE) }),
        );
        if (version !== generation || destroyed) return;
        for (const event of page.events) {
          received.add(event.id);
          appendLive(event);
        }
        cursor = page.events.at(-1)?.id ?? cursor;
        if (!page.has_more) break;
      }
      setError('');
      needsRecovery = false;
      succeeded = true;
    } catch (err) {
      if (version !== generation || destroyed) return;
      if (err instanceof ApiError && err.status === 410) {
        notify('保留期已更新，已载入最近记录');
        await resetHistory();
      } else {
        setError('记录同步暂时失败，正在自动重试');
        needsRecovery = true;
        clearTimeout(recoveryTimer);
        recoveryTimer = setTimeout(() => {
          void recover();
        }, 3000);
      }
    } finally {
      recovering = false;
      if (version === generation && succeeded) {
        const pending = buffered;
        buffered = [];
        for (const event of pending) if (!received.has(event.id)) appendLive(event);
      } else if (version === generation)
        buffered = buffered.filter((event) => !received.has(event.id));
      if (recoveryRequested && !loading() && !destroyed) {
        recoveryRequested = false;
        void recover();
      }
    }
  }

  async function loadOlder() {
    if (loadingOlder() || loading()) return;
    const first = events()[0];
    if (!first) return;
    const version = generation;
    const height = feed.scrollHeight,
      scroll = feed.scrollTop;
    setLoadingOlder(true);
    try {
      const page = await fetchHistory(
        new URLSearchParams({
          before: first.id,
          limit: String(PAGE_SIZE),
          kind: kind(),
          q: query(),
        }),
      );
      if (version !== generation || destroyed) return;
      setEvents((current) => mergeEvents(current, page.events, true));
      setHasMore(page.has_more);
      requestAnimationFrame(() => {
        feed.scrollTop = scroll + feed.scrollHeight - height;
      });
    } catch (err) {
      if (version === generation) setError(err instanceof Error ? err.message : '无法加载更早记录');
    } finally {
      setLoadingOlder(false);
    }
  }

  async function copy(value: string) {
    try {
      await navigator.clipboard.writeText(value);
      notify('已复制到剪贴板');
    } catch {
      notify('浏览器无法访问剪贴板，请手动选择文本复制');
    }
  }
  createEffect(() => {
    const value = search();
    const timer = setTimeout(() => setQuery(value.trim()), 300);
    onCleanup(() => clearTimeout(timer));
  });
  createEffect(
    on([kind, query], () => {
      void resetHistory();
    }),
  );
  createEffect(() => {
    if (settings()) dialog?.showModal();
    else if (dialog?.open) dialog.close();
  });

  createEffect(
    on(page, (value) => {
      if (value === 'chat' && following()) toBottom();
    }),
  );
  onMount(() => {
    void refreshOverview();
    readPlayerSnapshots();
    source = new EventSource('/api/events');
    source.onopen = () => {
      // A restarted backend begins a new revision sequence; discard pending old responses.
      playerConnection++;
      playersRevision = rankingsRevision = -1;
      setStreamState('open');
      void recover();
      void refreshOverview();
      readPlayerSnapshots();
    };
    source.onerror = () => setStreamState('closed');
    source.addEventListener('status', (event) => {
      try {
        const status = JSON.parse((event as MessageEvent<string>).data) as { connected: boolean };
        setConnected(status.connected === true);
      } catch {
        /* Ignore malformed control frames. */
      }
    });
    source.addEventListener('sync', () => {
      void recover();
    });
    source.addEventListener('player-sync', readPlayerSnapshots);
    source.addEventListener('players', (event) => {
      try {
        const value: unknown = JSON.parse((event as MessageEvent<string>).data);
        if (isPlayerSnapshot(value)) acceptPlayers(value as PlayerSnapshot);
      } catch {
        /* Recover through cached snapshot polling. */
      }
    });
    source.addEventListener('rankings', (event) => {
      try {
        const value: unknown = JSON.parse((event as MessageEvent<string>).data);
        if (isPlayerSnapshot(value, true)) acceptRankings(value as RankingSnapshot);
      } catch {
        /* Recover through cached snapshot polling. */
      }
    });
    source.onmessage = (event) => {
      try {
        const value: unknown = JSON.parse(event.data);
        if (!isChatEvent(value)) return;
        if (loading() || recovering || needsRecovery) buffered.push(value);
        else appendLive(value);
      } catch {
        setError('收到无法识别的实时记录，请刷新重试');
      }
    };
    const refresh = setInterval(() => {
      void refreshOverview();
    }, 30_000);
    const playerRefresh = setInterval(readPlayerSnapshots, 60_000);
    const keyboard = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement;
      if (
        event.key === '/' &&
        !['INPUT', 'TEXTAREA'].includes(target.tagName) &&
        !target.isContentEditable &&
        !settings() &&
        page() === 'chat'
      ) {
        event.preventDefault();
        searchInput.focus();
      }
      if (event.key === 'Escape') {
        setSelected(undefined);
        if (searchInput?.isConnected) searchInput.blur();
      }
    };
    window.addEventListener('keydown', keyboard);
    onCleanup(() => {
      clearInterval(refresh);
      clearInterval(playerRefresh);
      window.removeEventListener('keydown', keyboard);
    });
  });
  onCleanup(() => {
    destroyed = true;
    source?.close();
    controller?.abort();
    clearTimeout(toastTimer);
    clearTimeout(statsTimer);
    clearTimeout(recoveryTimer);
  });

  return (
    <div class="app-shell">
      <aside class="sidebar">
        <a class="brand" href="/" aria-label="RLMonitor 首页">
          <span class="brand-mark">
            <WorldMark />
          </span>
          <span>
            RLMonitor<span class="brand-caption">MINECRAFT CHAT</span>
          </span>
        </a>
        <div class="workspace-label">服务器</div>
        <button class="world-switch" onClick={() => setSettings(true)}>
          <span class="world-icon">
            <Sprout size={19} />
          </span>
          <span>
            <strong>{overview()?.server_name ?? 'Minecraft'}</strong>
            <small>连接与存储</small>
          </span>
          <ChevronDown size={14} />
        </button>
        <nav aria-label="主要导航">
          <button
            classList={{ active: page() === 'chat' }}
            aria-current={page() === 'chat' ? 'page' : undefined}
            onClick={() => setPage('chat')}
          >
            <MessageSquare size={18} />
            <span>聊天记录</span>
            <span class="nav-count">{number(stats()?.stored_messages ?? 0)}</span>
          </button>
          <button
            classList={{ active: page() === 'players' }}
            aria-current={page() === 'players' ? 'page' : undefined}
            onClick={() => setPage('players')}
          >
            <Users size={18} />
            <span>玩家</span>
            <Show when={players()?.updated_at}>
              <span class="nav-count">{number(onlineCount())} 在线</span>
            </Show>
          </button>
          <button
            classList={{ active: page() === 'rankings' }}
            aria-current={page() === 'rankings' ? 'page' : undefined}
            onClick={() => setPage('rankings')}
          >
            <Trophy size={18} />
            <span>排行榜</span>
          </button>
        </nav>
        <div class="sidebar-bottom">
          <button onClick={() => setSettings(true)}>
            <Settings2 size={17} />
            <span>连接与存储</span>
            <ChevronRight size={14} />
          </button>
          <a href="https://github.com/idkidknow/RealityLink" target="_blank" rel="noreferrer">
            <CircleHelp size={17} />
            <span>RealityLink 文档</span>
            <ArrowUpRight size={14} />
          </a>
          <div class="sidebar-status">
            <span
              classList={{ 'status-dot': true, online: connected() && streamState() === 'open' }}
            />
            <span>{connected() && streamState() === 'open' ? '实时记录已连接' : '等待连接'}</span>
            <span class="version">v0.2</span>
          </div>
        </div>
      </aside>

      <div class="workspace">
        <header class="topbar">
          <div class="breadcrumb">
            <span class="mobile-mark">
              <WorldMark />
            </span>
            <span>{overview()?.server_name ?? 'Minecraft'}</span>
            <ChevronRight size={13} />
            <strong>{pageTitle()}</strong>
          </div>
          <div class="topbar-right">
            <span class="topbar-date">
              {new Date().toLocaleDateString('zh-CN', {
                month: 'long',
                day: 'numeric',
                weekday: 'long',
              })}
            </span>
            <button
              class="icon-button mobile-settings"
              aria-label="连接与存储"
              onClick={() => setSettings(true)}
            >
              <Settings2 size={18} />
            </button>
          </div>
        </header>
        <main>
          <section class="page-heading">
            <div>
              <h1>{pageTitle()}</h1>
              <p>
                {page() === 'chat'
                  ? '搜索游戏消息、查看连接事件与原始消息。'
                  : page() === 'players'
                    ? '查看在线玩家与服务器缓存中的玩家。'
                    : '查看玩家累计游戏时长与死亡次数。'}
              </p>
            </div>
            <button
              class="button export-button"
              style={{ display: page() === 'chat' ? undefined : 'none' }}
              disabled={!events().length || loading()}
              onClick={() => {
                exportEvents(events());
                notify(`已导出当前载入的 ${events().length} 条记录`);
              }}
            >
              <ArrowDownToLine size={16} />
              导出已载入记录
            </button>
          </section>

          <nav class="mobile-page-nav" aria-label="移动端导航">
            <button
              classList={{ active: page() === 'chat' }}
              aria-current={page() === 'chat' ? 'page' : undefined}
              onClick={() => setPage('chat')}
            >
              <MessageSquare size={14} />
              聊天记录
            </button>
            <button
              classList={{ active: page() === 'players' }}
              aria-current={page() === 'players' ? 'page' : undefined}
              onClick={() => setPage('players')}
            >
              <Users size={14} />
              玩家
            </button>
            <button
              classList={{ active: page() === 'rankings' }}
              aria-current={page() === 'rankings' ? 'page' : undefined}
              onClick={() => setPage('rankings')}
            >
              <Trophy size={14} />
              排行榜
            </button>
          </nav>
          <Show when={page() === 'players'}>
            <PlayersPage
              directory={players()}
              error={playersError()}
              refreshing={refreshingPlayers()}
              onRefresh={() => {
                void readPlayerData('players', true);
              }}
              onSearch={searchPlayer}
              onCopy={(value) => {
                void copy(value);
              }}
            />
          </Show>
          <Show when={page() === 'rankings'}>
            <RankingsPage
              directory={players()}
              rankings={rankings()}
              error={rankingsError()}
              refreshing={refreshingRankings()}
              onRefresh={() => {
                void readPlayerData('rankings', true);
              }}
              onSearch={searchPlayer}
            />
          </Show>
          <Show when={page() === 'chat'}>
            <section class="metrics" aria-label="聊天统计">
              <div class="metric">
                <div class="metric-label">
                  过去 24 小时 <MessageSquare size={15} />
                </div>
                <div class="metric-value">
                  {overview() ? number(stats()!.messages_24h) : '—'}
                  <span>条消息</span>
                </div>
                <div class="metric-note">
                  最近消息
                  <time datetime={stats()?.last_message_at ?? undefined}>
                    {stats()?.last_message_at ? fullDate(stats()!.last_message_at!) : '暂无消息'}
                  </time>
                </div>
              </div>
              <div class="metric">
                <div class="metric-label">
                  已存档消息 <Database size={15} />
                </div>
                <div class="metric-value">
                  {overview() ? number(stats()!.stored_messages) : '—'}
                  <span>条消息</span>
                </div>
                <div class="metric-note">自动保留最近 {overview()?.retention_days ?? '—'} 天</div>
              </div>
              <div class="metric activity-metric">
                <div class="metric-label">
                  每小时消息量 <span class="tiny-mono">24H</span>
                </div>
                <div class="activity-chart" role="img" aria-label="最近 24 个小时的游戏消息数量">
                  <For
                    each={
                      stats()?.activity ??
                      Array.from({ length: 24 }, () => ({ hour: '', count: 0 }))
                    }
                  >
                    {(bar) => (
                      <span
                        classList={{ 'activity-bar': true, empty: bar.count === 0 }}
                        style={{ height: `${Math.max(3, (bar.count / activityMax()) * 37)}px` }}
                        title={
                          bar.hour ? `${timeLabel(bar.hour)} · ${bar.count} 条消息` : '暂无消息'
                        }
                      />
                    )}
                  </For>
                </div>
                <div class="activity-axis">
                  <span>23 小时前</span>
                  <span>当前小时</span>
                </div>
              </div>
            </section>

            <div class="content-grid">
              <section class="timeline" aria-label="聊天时间线">
                <div class="timeline-heading">
                  <h2>
                    记录列表
                    <span class="count-tag">{number(events().length)}</span>
                  </h2>
                  <div
                    class="live-label"
                    classList={{ offline: !connected() || streamState() !== 'open' }}
                  >
                    <span
                      class="status-dot"
                      classList={{ online: connected() && streamState() === 'open' }}
                    />
                    {streamState() !== 'open' ? '重新连接中' : connected() ? '实时' : '离线'}
                  </div>
                </div>
                <div class="timeline-tools">
                  <label class="search-box">
                    <Search size={17} />
                    <input
                      ref={searchInput}
                      value={search()}
                      onInput={(e) => setSearch(e.currentTarget.value)}
                      placeholder="搜索玩家或消息内容…"
                      aria-label="搜索玩家或消息内容"
                      maxlength={80}
                    />
                    <Show when={search()} fallback={<kbd>/</kbd>}>
                      <button
                        class="clear-search"
                        aria-label="清除搜索"
                        onClick={() => {
                          setSearch('');
                          searchInput.focus();
                        }}
                      >
                        <X size={14} />
                      </button>
                    </Show>
                  </label>
                  <button
                    class="icon-button refresh-button"
                    aria-label="刷新记录"
                    disabled={loading()}
                    onClick={() => {
                      void resetHistory();
                      void refreshOverview();
                    }}
                  >
                    <RefreshCw size={16} classList={{ spinning: loading() }} />
                  </button>
                </div>
                <div class="filter-row">
                  <div class="tabs" role="group" aria-label="事件类型">
                    <For
                      each={
                        [
                          ['all', '全部记录'],
                          ['chat', '游戏消息'],
                          ['connections', '连接事件'],
                        ] as const
                      }
                    >
                      {([value, label]) => (
                        <button
                          classList={{ active: kind() === value }}
                          aria-pressed={kind() === value}
                          onClick={() => setKind(value)}
                        >
                          {label}
                        </button>
                      )}
                    </For>
                  </div>
                  <button
                    class="follow-toggle"
                    role="switch"
                    onClick={() => (following() ? setFollowing(false) : follow())}
                    aria-checked={following()}
                    aria-label="自动跟随"
                  >
                    <span class="follow-switch" aria-hidden="true">
                      <span />
                    </span>
                    <span>自动跟随</span>
                  </button>
                </div>
                <Show when={error()}>
                  <div class="error-banner" role="alert">
                    <WifiOff size={16} />
                    <span>{error()}</span>
                    <button
                      onClick={() => {
                        void resetHistory();
                        void refreshOverview();
                      }}
                    >
                      重试
                    </button>
                  </div>
                </Show>

                <div
                  class="feed"
                  ref={feed}
                  tabindex="0"
                  aria-label="聊天记录，可滚动查看"
                  onScroll={() => {
                    if (following() && feed.scrollHeight - feed.scrollTop - feed.clientHeight > 90)
                      setFollowing(false);
                  }}
                >
                  <Show
                    when={!loading()}
                    fallback={
                      <div class="loading-state">
                        <LoaderCircle size={22} class="spinning" />
                        <span>正在加载记录…</span>
                        <div class="skeleton-row" />
                        <div class="skeleton-row short" />
                        <div class="skeleton-row" />
                      </div>
                    }
                  >
                    <Show when={hasMore()}>
                      <div class="older-wrap">
                        <button
                          class="older-button"
                          disabled={loadingOlder()}
                          onClick={() => {
                            setFollowing(false);
                            void loadOlder();
                          }}
                        >
                          {loadingOlder() ? (
                            <LoaderCircle size={13} class="spinning" />
                          ) : (
                            <BookOpen size={13} />
                          )}{' '}
                          {loadingOlder() ? '正在加载…' : '加载更早记录'}
                        </button>
                      </div>
                    </Show>
                    <Show
                      when={events().length}
                      fallback={
                        <div class="empty-state">
                          <span class="empty-icon">
                            <MessageSquare size={25} />
                          </span>
                          <h3>
                            {query()
                              ? '没有匹配的记录'
                              : kind() === 'connections'
                                ? '还没有连接记录'
                                : '暂无聊天记录'}
                          </h3>
                          <p>
                            {query()
                              ? '尝试其他关键词，或清除筛选。'
                              : connected()
                                ? '收到新消息后，记录将自动显示在这里。'
                                : '等待 RealityLink 连接后接收游戏内消息。'}
                          </p>
                          <Show when={query() || kind() !== 'all'}>
                            <button
                              class="button"
                              onClick={() => {
                                setSearch('');
                                setKind('all');
                              }}
                            >
                              清除筛选
                            </button>
                          </Show>
                        </div>
                      }
                    >
                      <For each={groups()}>
                        {(group) => (
                          <div class="day-group">
                            <div class="day-divider">
                              <span>{group.label}</span>
                              <span class="day-date">{group.day.replaceAll('-', '.')}</span>
                            </div>
                            <For each={group.items}>
                              {(event) => {
                                const view = messageView(event);
                                const longMessage =
                                  view.text.length > 280 || view.text.split('\n').length > 6;
                                return (
                                  <Show
                                    when={event.event_type === 'chat'}
                                    fallback={
                                      <button
                                        class="connection-row"
                                        data-event-id={event.id}
                                        classList={{ selected: selected()?.id === event.id }}
                                        onClick={() => setSelected(event)}
                                      >
                                        <span
                                          classList={{
                                            'connection-symbol': true,
                                            disconnected: event.event_type === 'disconnected',
                                          }}
                                        >
                                          {event.event_type === 'connected' ? (
                                            <Wifi size={13} />
                                          ) : (
                                            <WifiOff size={13} />
                                          )}
                                        </span>
                                        <span>
                                          {event.event_type === 'connected'
                                            ? 'RealityLink 已连接'
                                            : '与 RealityLink 的连接断开，正在尝试重连'}
                                        </span>
                                        <time class="row-time" datetime={event.timestamp}>
                                          {timeLabel(event.timestamp)}
                                        </time>
                                        <ChevronRight class="row-arrow" size={14} />
                                      </button>
                                    }
                                  >
                                    <button
                                      class="message-row"
                                      data-event-id={event.id}
                                      classList={{
                                        selected: selected()?.id === event.id,
                                        'long-message': longMessage,
                                      }}
                                      onClick={() => setSelected(event)}
                                      aria-label={`查看 ${view.player ?? '系统'} 的消息：${view.text}`}
                                    >
                                      <Show
                                        when={view.player}
                                        fallback={
                                          <span class="avatar system-avatar">
                                            <Hash size={17} />
                                          </span>
                                        }
                                      >
                                        {(player) => <Avatar name={player()} />}
                                      </Show>
                                      <div class="message-body">
                                        <div class="message-meta">
                                          <strong>{view.player ?? '系统'}</strong>
                                          <Show when={view.kind !== 'chat'}>
                                            <span class="message-tag" data-kind={view.kind}>
                                              {view.label}
                                            </span>
                                          </Show>
                                        </div>
                                        <p>{view.text}</p>
                                        <Show when={longMessage}>
                                          <span class="message-more">
                                            查看完整消息 <ChevronRight size={11} />
                                          </span>
                                        </Show>
                                      </div>
                                      <time class="row-time" datetime={event.timestamp}>
                                        {timeLabel(event.timestamp)}
                                      </time>
                                      <ChevronRight class="row-arrow" size={14} />
                                    </button>
                                  </Show>
                                );
                              }}
                            </For>
                          </div>
                        )}
                      </For>
                      <div class="end-note">
                        <span /> 已经是最新的记录 <span />
                      </div>
                    </Show>
                  </Show>
                </div>
                <Show when={!following() && unread() > 0}>
                  <button class="new-messages" onClick={follow}>
                    <ArrowDown size={14} />
                    {unread()} 条新记录 · 回到最新
                  </button>
                </Show>
                <footer class="timeline-footer">
                  <span class="footer-status">
                    <span
                      class="status-dot"
                      classList={{ online: streamState() === 'open' && connected() }}
                    />
                    {statusText()}
                  </span>
                  <span class="follow-status">
                    {following() ? '自动跟随最新记录' : '已暂停跟随'}
                  </span>
                </footer>
              </section>

              <aside
                class="inspector"
                classList={{ 'has-selection': !!selected() }}
                aria-label={selected() ? '记录详情' : '记录概览'}
              >
                <Show
                  when={selected()}
                  fallback={
                    <>
                      <section class="server-panel" aria-label="服务器状态">
                        <h2 class="section-kicker">
                          <Wifi size={15} /> 服务器状态
                        </h2>
                        <dl class="connection-details">
                          <dt>记录服务</dt>
                          <dd classList={{ ready: streamState() === 'open' }}>
                            <span
                              class="status-dot"
                              classList={{ online: streamState() === 'open' }}
                            />
                            {streamState() === 'open' ? '已连接' : '连接中'}
                          </dd>
                          <dt>RealityLink</dt>
                          <dd classList={{ ready: connected() && streamState() === 'open' }}>
                            <span
                              class="status-dot"
                              classList={{ online: connected() && streamState() === 'open' }}
                            />
                            {streamState() !== 'open'
                              ? '状态未知'
                              : connected()
                                ? '已连接'
                                : '等待连接'}
                          </dd>
                        </dl>
                      </section>
                      <section class="results-panel" aria-label="当前载入记录统计">
                        <h2 class="section-kicker">
                          <Activity size={15} /> 当前载入 <span>{number(events().length)} 条</span>
                        </h2>
                        <div class="result-counts">
                          <div>
                            <strong>{number(loadedSummary().chatCount)}</strong>
                            <span>游戏消息</span>
                          </div>
                          <div>
                            <strong>{number(loadedSummary().connectionCount)}</strong>
                            <span>连接事件</span>
                          </div>
                        </div>
                        <Show when={loadedSummary().players.length}>
                          <div class="players-heading">
                            <h3>相关玩家</h3>
                            <span>{number(loadedSummary().players.length)} 位玩家</span>
                          </div>
                          <div class="player-list">
                            <For each={loadedSummary().players.slice(0, 6)}>
                              {(player) => (
                                <button
                                  class="player-search"
                                  title={`搜索包含 ${player.name} 的游戏消息`}
                                  aria-label={`搜索包含 ${player.name} 的游戏消息`}
                                  onClick={() => {
                                    batch(() => {
                                      setKind('chat');
                                      setSearch(player.name);
                                      setQuery(player.name.trim());
                                    });
                                  }}
                                >
                                  <Avatar name={player.name} />
                                  <span>{player.name}</span>
                                  <strong>{number(player.count)}</strong>
                                  <Search size={13} />
                                </button>
                              )}
                            </For>
                          </div>
                          <Show when={loadedSummary().players.length > 6}>
                            <p class="results-note">显示消息最多的 6 位玩家</p>
                          </Show>
                        </Show>
                      </section>
                    </>
                  }
                >
                  {(event) => (
                    <div class="detail-panel">
                      <div class="detail-heading">
                        <span class="detail-title">记录详情</span>
                        <button
                          class="icon-button"
                          aria-label="关闭消息详情"
                          onClick={() => setSelected(undefined)}
                        >
                          <X size={17} />
                        </button>
                      </div>
                      <div class="detail-author">
                        <Show
                          when={messageView(event()).player}
                          fallback={
                            <span class="avatar system-avatar">
                              <Hash size={19} />
                            </span>
                          }
                        >
                          {(player) => <Avatar name={player()} />}
                        </Show>
                        <div>
                          <h2>
                            {messageView(event()).player ??
                              (event().event_type === 'chat' ? '系统' : 'RealityLink')}
                          </h2>
                          <p>
                            {event().event_type === 'chat'
                              ? messageView(event()).label
                              : '连接事件'}
                          </p>
                        </div>
                      </div>
                      <div class="detail-content">
                        {event().event_type === 'chat'
                          ? messageView(event()).text
                          : event().event_type === 'connected'
                            ? 'RealityLink 已连接'
                            : 'RealityLink 连接已断开'}
                      </div>
                      <dl class="detail-metadata">
                        <dt>记录时间</dt>
                        <dd>{fullDate(event().timestamp)}</dd>
                        <dt>事件类型</dt>
                        <dd class="tiny-mono">{event().event_type}</dd>
                        <dt>记录 ID</dt>
                        <dd class="event-id">{event().id}</dd>
                      </dl>
                      <button
                        class="button copy-message"
                        onClick={() => {
                          void copy(event().content ?? event().event_type);
                        }}
                      >
                        <Copy size={14} />
                        复制消息
                      </button>
                      <div class="raw-heading">
                        <h3>
                          <FileJson size={15} /> 原始 Minecraft 消息
                        </h3>
                        <button
                          class="icon-button"
                          aria-label="复制原始 JSON"
                          onClick={() => {
                            void copy(prettyJson(event().raw_json));
                          }}
                        >
                          <Copy size={14} />
                        </button>
                      </div>
                      <pre class="raw-json">
                        <code>{prettyJson(event().raw_json)}</code>
                      </pre>
                      <p class="raw-note">保留原始消息，方便回溯内容与格式。</p>
                      <button class="detail-back" onClick={() => setSelected(undefined)}>
                        <ArrowLeft size={14} /> 返回记录概览
                      </button>
                    </div>
                  )}
                </Show>
              </aside>
            </div>
          </Show>
          <footer class="page-footer">
            <span>
              <WorldMark /> RLMonitor <span class="footer-separator">/</span> Minecraft 聊天记录
            </span>
            <a href="https://github.com/idkidknow/RealityLink" target="_blank" rel="noreferrer">
              RealityLink 文档 <ArrowUpRight size={12} />
            </a>
          </footer>
        </main>
      </div>
      <dialog
        ref={dialog}
        aria-label="连接与存储"
        class="settings-dialog"
        onClose={() => setSettings(false)}
        onCancel={() => setSettings(false)}
        onClick={(e) => {
          if (e.target === dialog) {
            const r = dialog.getBoundingClientRect();
            if (
              e.clientX < r.left ||
              e.clientX > r.right ||
              e.clientY < r.top ||
              e.clientY > r.bottom
            )
              setSettings(false);
          }
        }}
      >
        <div class="dialog-heading">
          <div>
            <h2>连接与存储</h2>
          </div>
          <button class="icon-button" aria-label="关闭设置" onClick={() => setSettings(false)}>
            <X size={19} />
          </button>
        </div>
        <div class="settings-world">
          <span class="world-icon">
            <Sprout size={21} />
          </span>
          <div>
            <strong>{overview()?.server_name ?? 'Minecraft'}</strong>
            <p>{statusText()}</p>
          </div>
        </div>
        <dl class="settings-list">
          <dt>记录服务</dt>
          <dd>{streamState() === 'open' ? '已连接' : '正在重新连接'}</dd>
          <dt>RealityLink</dt>
          <dd>{streamState() !== 'open' ? '状态未知' : connected() ? '已连接' : '等待连接'}</dd>
          <dt>连接地址</dt>
          <dd class="settings-address">{overview()?.realitylink_address ?? '—'}</dd>
          <dt>自动保留</dt>
          <dd>{overview()?.retention_days ?? '—'} 天</dd>
          <dt>本地存储</dt>
          <dd>SQLite</dd>
        </dl>
        <p class="settings-help">
          连接地址、服务器名称与保留时长由服务端配置管理。记录会自动保存，并定期清理超过保留期的内容。
        </p>
        <a
          class="settings-link"
          href="https://github.com/idkidknow/RealityLink"
          target="_blank"
          rel="noreferrer"
        >
          查看 RealityLink 文档 <ArrowUpRight size={14} />
        </a>
        <button class="button primary dialog-done" onClick={() => setSettings(false)}>
          关闭
        </button>
      </dialog>
      <Show when={toast()}>
        <div class="toast" role="status">
          <Check size={16} />
          {toast()}
        </div>
      </Show>
    </div>
  );
}
