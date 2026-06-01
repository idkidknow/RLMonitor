import test from 'node:test';
import assert from 'node:assert/strict';
import { componentText, messageView, mergeEvents, matches, isChatEvent } from './lib.ts';
import type { ChatEvent } from './lib.ts';

const event: ChatEvent = {
  id: '1',
  timestamp: '2026-10-02T13:00:00Z',
  event_type: 'chat',
  content: '<Alex> §a你好 <script>alert(1)</script>',
  raw_json: '{"text":"hello"}',
};
test('extracts the speaker and preserves text safely for JSX', () => {
  assert.deepEqual(messageView(event), {
    player: 'Alex',
    players: ['Alex'],
    text: '你好 <script>alert(1)</script>',
    kind: 'chat',
    label: '玩家聊天',
  });
  assert.equal(componentText({ text: 'A', extra: [{ text: 'B' }, 'C'] }), 'ABC');
  assert.equal(messageView({ ...event, content: event.raw_json }).text, 'hello');
});
test('identifies players in translated game notices without treating mobs as players', () => {
  const player = (name: string) => ({
    text: '',
    extra: [{ text: name }],
    insertion: name,
    hoverEvent: {
      action: 'show_entity',
      contents: { type: 'minecraft:player', name: { text: name } },
    },
  });
  const mob = {
    insertion: 'entity-uuid',
    hoverEvent: {
      action: 'show_entity',
      contents: { type: 'minecraft:zombie', name: { text: 'Zombie' } },
    },
  };
  for (const [translate, kind, label] of [
    ['multiplayer.player.joined', 'joined', '加入游戏'],
    ['multiplayer.player.left', 'left', '退出游戏'],
    ['chat.type.advancement.task', 'advancement', '进度'],
    ['chat.type.advancement.challenge', 'advancement', '进度'],
    ['chat.type.advancement.goal', 'advancement', '进度'],
    ['death.attack.mob', 'death', '死亡'],
    ['death.attack.example_mod.magic', 'death', '死亡'],
  ]) {
    const view = messageView({
      ...event,
      content: 'Alex §e游戏通知',
      raw_json: JSON.stringify({ translate, with: [player('Alex'), mob] }),
    });
    assert.deepEqual(view, {
      player: 'Alex',
      players: ['Alex'],
      text: 'Alex 游戏通知',
      kind,
      label,
    });
  }
  const pvp = messageView({
    ...event,
    content: 'Alex 被 Steve 杀死',
    raw_json: JSON.stringify({
      translate: 'death.attack.player',
      with: [player('Alex'), player('Steve'), player('Alex')],
    }),
  });
  assert.deepEqual(pvp.players, ['Alex', 'Steve']);
  assert.equal(pvp.player, 'Alex');
});
test('keeps unclassified system notices and malformed raw data readable', () => {
  assert.equal(
    messageView({ ...event, content: '服务器通知', raw_json: '{broken' }).text,
    '服务器通知',
  );
  assert.equal(
    messageView({ ...event, content: '服务器通知', raw_json: '{broken' }).kind,
    'system',
  );
  const translated = messageView({
    ...event,
    content: 'Alex 你好',
    raw_json: JSON.stringify({
      translate: 'chat.type.text',
      with: [{ text: 'Alex' }, { text: '你好' }],
    }),
  });
  assert.equal(translated.player, 'Alex');
  assert.equal(translated.kind, 'chat');
  assert.equal(translated.text, 'Alex 你好');
});
test('recovery deduplicates IDs without sorting opaque legacy cursors', () => {
  const next = { ...event, id: '0' };
  assert.deepEqual(
    mergeEvents([event], [event, next]).map((e) => e.id),
    ['1', '0'],
  );
  assert.deepEqual(
    mergeEvents([next], [event, next], true).map((e) => e.id),
    ['1', '0'],
  );
});
test('filters literal search text and validates SSE payloads', () => {
  assert.equal(matches(event, 'chat', 'alex'), true);
  assert.equal(matches(event, 'connections', ''), false);
  assert.equal(matches(event, 'all', '%'), false);
  assert.equal(isChatEvent(event), true);
  assert.equal(isChatEvent({ ...event, event_type: 'unknown' }), false);
  assert.equal(isChatEvent({ id: '0', synthetic: true }), false);
});

test('ranks ties correctly, includes real zero values and excludes unavailable stats', async () => {
  const { rankPlayers, playTimeLabel, isPlayerSnapshot } = await import('./lib.ts');
  const row = (name: string, time: number | null, deaths: number | null) => ({
    uuid: name,
    name,
    online: false,
    play_time_ticks: time,
    deaths,
    updated_at: null,
    error: false,
  });
  const players = [
    row('Cedar', 0, 0),
    row('Birch', 72000, 7),
    row('Alex', 72000, 2),
    row('Unknown', null, null),
  ];
  assert.deepEqual(
    rankPlayers(players, 'play_time_ticks').map((r) => [r.player.name, r.rank, r.value]),
    [
      ['Alex', 1, 72000],
      ['Birch', 1, 72000],
      ['Cedar', 3, 0],
    ],
  );
  assert.deepEqual(
    rankPlayers(players, 'deaths').map((r) => [r.player.name, r.rank]),
    [
      ['Birch', 1],
      ['Alex', 2],
      ['Cedar', 3],
    ],
  );
  assert.deepEqual(
    players.map((p) => p.name),
    ['Cedar', 'Birch', 'Alex', 'Unknown'],
    'Ranking must not mutate the source',
  );
  for (const [ticks, label] of [
    [0, '0 秒'],
    [1199, '59 秒'],
    [1200, '1 分钟'],
    [72000, '1 小时'],
    [73200, '1 小时 1 分钟'],
    [72000000, '1,000 小时'],
  ] as const)
    assert.equal(playTimeLabel(ticks), label);
  const snapshot = { players, updated_at: null, error: null, refreshing: false, revision: 0 };
  assert.equal(isPlayerSnapshot(snapshot, true), true);
  assert.equal(isPlayerSnapshot({ ...snapshot, revision: -1 }, true), false);
  assert.equal(isPlayerSnapshot({ ...snapshot, updated_at: 'invalid' }, true), false);
  assert.equal(isPlayerSnapshot({ ...snapshot, players: [row('Alex', 0, -1)] }, true), false);
  assert.equal(
    isPlayerSnapshot({ ...snapshot, players: [row('Alex', Number.MAX_SAFE_INTEGER + 1, 0)] }, true),
    false,
  );
});
