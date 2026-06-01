import { For, Show, createMemo, createSignal } from 'solid-js';
import {
  ArrowUpRight,
  Clock3,
  Copy,
  RefreshCw,
  Search,
  Skull,
  Trophy,
  Users,
  WifiOff,
} from 'lucide-solid';
import { Avatar } from './components';
import { fullDate, playTimeLabel, rankPlayers, timeLabel } from './lib';
import type { PlayerSnapshot, RankingSnapshot } from './lib';

const count = (value: number) => value.toLocaleString('zh-CN');
type SharedProps = {
  directory?: PlayerSnapshot;
  error: string;
  refreshing: boolean;
  onRefresh: () => void;
};

function RefreshInfo(props: {
  updated?: string | null;
  refreshing: boolean;
  onRefresh: () => void;
  label: string;
}) {
  return (
    <div class="data-refresh">
      <span>
        {props.updated ? (
          <>
            更新于{' '}
            <time datetime={props.updated} title={fullDate(props.updated)}>
              {timeLabel(props.updated)}
            </time>
          </>
        ) : (
          '尚未更新'
        )}
      </span>
      <button
        class="button data-refresh-button"
        disabled={props.refreshing}
        onClick={props.onRefresh}
        aria-label={props.label}
      >
        <RefreshCw size={14} classList={{ spinning: props.refreshing }} />
        {props.refreshing ? '正在刷新' : '刷新'}
      </button>
    </div>
  );
}
function DataError(props: { text: string }) {
  return (
    <Show when={props.text}>
      <div class="error-banner" role="alert">
        <WifiOff size={15} />
        <span>{props.text}</span>
      </div>
    </Show>
  );
}
function Presence(props: { online: boolean }) {
  return (
    <span class="player-presence" classList={{ online: props.online }}>
      <span class="status-dot" classList={{ online: props.online }} />
      {props.online ? '在线' : '离线'}
    </span>
  );
}

export function PlayersPage(
  props: SharedProps & { onSearch: (name: string) => void; onCopy: (text: string) => void },
) {
  const [scope, setScope] = createSignal<'online' | 'all'>('online');
  const [query, setQuery] = createSignal('');
  const onlineCount = () => props.directory?.players.filter((p) => p.online).length ?? 0;
  const rows = createMemo(() =>
    (props.directory?.players ?? []).filter(
      (p) =>
        (scope() === 'all' || p.online) &&
        `${p.name} ${p.uuid}`.toLowerCase().includes(query().trim().toLowerCase()),
    ),
  );
  const ready = () => !!props.directory?.updated_at;
  return (
    <section class="players-workspace" aria-label="玩家列表">
      <div class="player-summary">
        <div>
          <span class="metric-label">
            <span class="status-dot online" /> 在线玩家
          </span>
          <strong>{ready() ? count(onlineCount()) : '—'}</strong>
          <span class="summary-unit">人</span>
        </div>
        <div>
          <span class="metric-label">
            <Users size={14} /> 所有玩家
          </span>
          <strong>{ready() ? count(props.directory!.players.length) : '—'}</strong>
          <span class="summary-unit">人</span>
        </div>
        <RefreshInfo
          updated={props.directory?.updated_at}
          refreshing={props.refreshing || !!props.directory?.refreshing}
          onRefresh={props.onRefresh}
          label="立即刷新玩家列表"
        />
      </div>
      <div class="player-directory panel">
        <div class="directory-tools">
          <div class="tabs" role="group" aria-label="玩家范围">
            <button
              classList={{ active: scope() === 'online' }}
              aria-pressed={scope() === 'online'}
              onClick={() => setScope('online')}
            >
              在线玩家 <span>{ready() ? count(onlineCount()) : '—'}</span>
            </button>
            <button
              classList={{ active: scope() === 'all' }}
              aria-pressed={scope() === 'all'}
              onClick={() => setScope('all')}
            >
              所有玩家 <span>{ready() ? count(props.directory!.players.length) : '—'}</span>
            </button>
          </div>
          <label class="search-box">
            <Search size={15} />
            <input
              aria-label="搜索玩家名称或 UUID"
              placeholder="搜索玩家名称或 UUID…"
              value={query()}
              onInput={(e) => setQuery(e.currentTarget.value)}
            />
          </label>
        </div>
        <DataError text={props.error || props.directory?.error || ''} />
        <div class="directory-scroll">
          <Show
            when={rows().length}
            fallback={
              <div class="empty-state">
                <span class="empty-icon">
                  <Users size={25} />
                </span>
                <h3>
                  {!ready()
                    ? props.directory?.error || props.error
                      ? '玩家列表暂不可用'
                      : '正在读取玩家列表…'
                    : query()
                      ? '没有匹配的玩家'
                      : scope() === 'online'
                        ? '当前没有在线玩家'
                        : '服务器缓存中暂无玩家'}
                </h3>
                <p>{ready() ? '可切换玩家范围或手动刷新列表。' : '数据由 RealityLink 提供。'}</p>
              </div>
            }
          >
            <table class="directory-table">
              <thead>
                <tr>
                  <th scope="col">玩家</th>
                  <th scope="col">状态</th>
                  <th scope="col" class="uuid-column">
                    UUID
                  </th>
                  <th scope="col">
                    <span class="sr-only">操作</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                <For each={rows()}>
                  {(player) => (
                    <tr data-player-uuid={player.uuid}>
                      <td>
                        <div class="directory-player">
                          <Avatar name={player.name} />
                          <div>
                            <strong>{player.name}</strong>
                            <button
                              class="mobile-player-uuid"
                              onClick={() => props.onCopy(player.uuid)}
                              aria-label={`复制 ${player.name} 的 UUID`}
                              title={player.uuid}
                            >
                              {player.uuid.slice(0, 8)}…{player.uuid.slice(-8)}
                            </button>
                          </div>
                        </div>
                      </td>
                      <td>
                        <Presence online={player.online} />
                      </td>
                      <td class="uuid-column">
                        <button
                          class="copy-uuid"
                          onClick={() => props.onCopy(player.uuid)}
                          aria-label={`复制 ${player.name} 的 UUID`}
                        >
                          <span>{player.uuid}</span>
                          <Copy size={12} />
                        </button>
                      </td>
                      <td>
                        <button
                          class="player-history"
                          onClick={() => props.onSearch(player.name)}
                          aria-label={`查看 ${player.name} 的聊天记录`}
                          title="查看聊天记录"
                        >
                          <span>查看记录</span>
                          <ArrowUpRight size={14} />
                        </button>
                      </td>
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
          </Show>
        </div>
        <footer class="directory-footer">
          <span>{ready() ? `显示 ${count(rows().length)} 位玩家` : '等待玩家数据'}</span>
          <span>所有玩家包含在线玩家及服务器缓存玩家</span>
        </footer>
      </div>
    </section>
  );
}

export function RankingsPage(
  props: SharedProps & { rankings?: RankingSnapshot; onSearch: (name: string) => void },
) {
  const [expanded, setExpanded] = createSignal(false);
  const currentPlayers = createMemo(
    () => new Map((props.directory?.players ?? []).map((p) => [p.uuid, p])),
  );
  const items = () =>
    (props.rankings?.players ?? []).map((row) => ({
      ...row,
      ...(currentPlayers().get(row.uuid) ?? {}),
    }));
  const sections = createMemo(() => [
    {
      metric: 'play_time_ticks' as const,
      title: '游戏时长',
      icon: Clock3,
      unit: '累计游戏时长',
      rows: rankPlayers(items(), 'play_time_ticks'),
    },
    {
      metric: 'deaths' as const,
      title: '死亡次数',
      icon: Skull,
      unit: '累计死亡次数',
      rows: rankPlayers(items(), 'deaths'),
    },
  ]);
  return (
    <section class="rankings-workspace" aria-label="玩家排行榜">
      <div class="ranking-toolbar">
        <div>
          <Trophy size={16} />
          <span>累计统计</span>
          <span class="ranking-scope">
            {props.directory?.updated_at
              ? `${count(props.directory.players.length)} 位玩家`
              : '等待玩家数据'}
          </span>
        </div>
        <RefreshInfo
          updated={props.rankings?.updated_at}
          refreshing={props.refreshing || !!props.rankings?.refreshing}
          onRefresh={props.onRefresh}
          label="立即刷新排行榜"
        />
      </div>
      <DataError text={props.error || props.rankings?.error || ''} />
      <div class="ranking-scroll">
        <div class="ranking-grid">
          <For each={sections()}>
            {(section) => (
              <section class="ranking-card panel" aria-label={`${section.title}排行榜`}>
                <div class="ranking-heading">
                  <span class="ranking-icon">
                    <section.icon size={19} />
                  </span>
                  <div>
                    <h2>{section.title}</h2>
                    <p>{section.unit}</p>
                  </div>
                  <span>{count(section.rows.length)} 人</span>
                </div>
                <Show
                  when={section.rows.length}
                  fallback={
                    <div class="empty-state">
                      <span class="empty-icon">
                        <section.icon size={25} />
                      </span>
                      <h3>
                        {props.rankings?.refreshing
                          ? '正在读取统计…'
                          : props.rankings?.error || props.error
                            ? '统计暂不可用'
                            : '暂无可用统计'}
                      </h3>
                      <p>可手动刷新后重试。</p>
                    </div>
                  }
                >
                  <ol class="ranking-list">
                    <For each={expanded() ? section.rows : section.rows.slice(0, 10)}>
                      {(row) => (
                        <li data-player-uuid={row.player.uuid} data-rank={row.rank}>
                          <span class="rank-number" classList={{ 'rank-leading': row.rank <= 3 }}>
                            {String(row.rank).padStart(2, '0')}
                          </span>
                          <button
                            class="ranking-player"
                            onClick={() => props.onSearch(row.player.name)}
                            aria-label={`查看 ${row.player.name} 的聊天记录`}
                          >
                            <Avatar name={row.player.name} />
                            <span>
                              <strong>{row.player.name}</strong>
                              <span class="rank-player-meta">
                                <Presence online={row.player.online} />
                                <Show when={row.player.error}>
                                  <span
                                    class="rank-stale"
                                    title={
                                      row.player.updated_at
                                        ? `上次取得完整统计：${fullDate(row.player.updated_at)}`
                                        : '本次统计未能完整读取'
                                    }
                                  >
                                    上次数据
                                  </span>
                                </Show>
                              </span>
                            </span>
                          </button>
                          <strong
                            class="rank-value"
                            title={
                              section.metric === 'play_time_ticks'
                                ? `${count(row.value)} 游戏刻，按 20 游戏刻/秒换算`
                                : undefined
                            }
                          >
                            {section.metric === 'play_time_ticks' ? (
                              playTimeLabel(row.value)
                            ) : (
                              <>
                                {count(row.value)}
                                <small>次</small>
                              </>
                            )}
                          </strong>
                        </li>
                      )}
                    </For>
                  </ol>
                </Show>
                <footer class="rank-card-footer">
                  <span>数值相同并列排名</span>
                  <Show when={items().length > section.rows.length}>
                    <span>{count(items().length - section.rows.length)} 位玩家暂无此项统计</span>
                  </Show>
                </footer>
              </section>
            )}
          </For>
        </div>
        <Show when={sections().some((s) => s.rows.length > 10)}>
          <button class="button ranking-expand" onClick={() => setExpanded((v) => !v)}>
            {expanded() ? '仅显示前 10 名' : '显示完整排行榜'}
          </button>
        </Show>
        <p class="ranking-note">
          游戏时长按 20 游戏刻 / 秒换算。排行榜基于服务器缓存玩家，统计不可用的玩家不参与对应排名。
        </p>
      </div>
    </section>
  );
}
