# RLMonitor

Minecraft 服务器聊天记录与玩家统计监控。支持聊天存储与检索、在线／缓存玩家列表，以及游戏时长和死亡次数排行榜。需要服务器安装 [RealityLink 1.0.0](https://github.com/idkidknow/RealityLink)。

## 运行

需要 Rust 1.99.0、Node.js 22.18+ 和 pnpm 12.3.4。Nix 用户可使用 `nix develop`。

在项目根目录执行，启动前在 `.env` 中填写 RealityLink 地址：

```sh
cp .env.example .env
pnpm --dir frontend install --frozen-lockfile
pnpm --dir frontend build
cargo build --release --locked
./target/release/rlmonitor
```

默认访问 `http://127.0.0.1:3000`。部署时需保留 `frontend/dist`。

## 配置

自动读取启动目录的 `.env`，进程环境变量优先。相对路径以启动目录为基准。

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `REALITYLINK_ADDRESS` | `localhost:39244` | RealityLink 的 `host:port`，不含协议或路径；IPv6 使用 `[::1]:39244` |
| `LISTEN_ADDR` | `127.0.0.1:3000` | HTTP 监听地址 |
| `DATABASE_PATH` | `chat_history.db` | SQLite 数据库路径 |
| `FRONTEND_DIR` | `frontend/dist` | 前端构建产物目录 |
| `SERVER_NAME` | `Minecraft` | 服务器显示名称 |
| `RETENTION_DAYS` | `3` | 聊天记录保留天数，范围 1–3650；过期记录自动删除 |
| `RUST_LOG` | `info` | 日志过滤级别 |

## 开发

后端在项目根目录启动：

```sh
cargo run
```

另一个终端启动前端：

```sh
pnpm --dir frontend install --frozen-lockfile
pnpm --dir frontend dev
```

访问 `http://127.0.0.1:5173`。开发服务器将 `/api` 代理到 `http://127.0.0.1:3000`，可通过 `API_PROXY_TARGET` 修改。

## 检查

```sh
cargo fmt --check
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
pnpm --dir frontend check
pnpm --dir frontend test
pnpm --dir frontend build
```
