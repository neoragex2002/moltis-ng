# 面向 V3 设计的配置重置与恢复

本文档是面向 V3 设计的操作说明，适用于直接切到 V3 的场景，不讨论兼容旧配置、旧目录或旧字段的平滑迁移。

目标只有两个：

- 把当前实例安全地重置成纯 V3 状态
- 保留恢复运行所需的最小配置与素材

## 目录与现状

默认情况下：

- 配置目录：`~/.moltis/config`
- 数据目录：`~/.moltis/data`

对当前这套环境，真正有价值的数据主要分成四类：

- `~/.moltis/config/moltis.toml`
- `~/.moltis/data/USER.md`
- `~/.moltis/data/PEOPLE.md`
- `~/.moltis/data/agents/`

此外，这个环境下 `~/.moltis/data/skills/` 不是空目录，因此如果你依赖这些技能，也应纳入备份。

## 最小备份集

如果你要做一次“可完全重建、但不保留旧数据库垃圾”的重置，建议至少备份：

- `~/.moltis/config/moltis.toml`
- `~/.moltis/data/USER.md`
- `~/.moltis/data/PEOPLE.md`
- `~/.moltis/data/agents/`
- `~/.moltis/data/skills/`

如果你不打算保留 `~/.moltis/data/moltis.db`，那还必须额外保存每个 Telegram bot 的完整渠道配置：

- `token`
- `chan_user_name`
- `chan_nickname`
- `allowlist`
- `agent_id`

这组字段决定了 bot 是否能被完整重建，不只是“记住 bot 账号名”。

## 可以不备份的内容

以下内容通常不是“V3 重置后恢复运行”的最小必需项：

- `~/.moltis/data/HEARTBEAT.md`
- `~/.moltis/data/memory.db`
- `~/.moltis/data/metrics.db`
- `~/.moltis/data/logs.jsonl`
- `~/.moltis/data/.onboarded`
- `~/.moltis/config/moltis.toml.bak`
- `~/.moltis/config/moltis.toml.default`

其中：

- `HEARTBEAT.md` 目前通常只是模板/注释占位
- `memory.db`、`metrics.db` 属于运行期数据，不是启动 Telegram 渠道和 agent 的必需配置

## 按需备份的内容

以下内容是否保留，取决于你是否还需要沿用对应状态：

- `~/.moltis/config/kimi_device_id`
  - 仅当你还想保留 Moonshot/Kimi 设备登录态时才需要
- `~/.moltis/config/certs/`
  - 仅当你想保留当前本地 TLS 证书与信任链时才需要

## 当前数据库里的过时内容

当前 `~/.moltis/data/moltis.db` 里，Telegram 渠道配置仍然带有旧字段内容：

- `persona_id`
- `group_session_transcript_format`
- `agent_id = null`

这类内容属于“旧 JSON 配置残留”，不是你想要的纯 V3 状态。

现在的代码策略是：

- 不再兼容旧的 `persona_id`
- 不再回退读取旧的 `people/` 目录
- Telegram 运行时热更新会在写回前剥离已删除字段，避免审批流程继续被旧字段卡住

换句话说，推荐做法不是继续修旧数据，而是把配置源收敛干净。

## TOML 与数据库的职责边界

当前启动路径里：

- `moltis.toml` 中的 `channels.telegram` 会先启动
- 数据库里持久化的 Telegram 渠道会随后加载
- 如果同一个 bot 同时存在于 TOML 和数据库，TOML 配置优先

这意味着：

- 如果 Telegram bot 只存在于数据库里，那么删掉 `moltis.db` 后，这些 bot 不会自动恢复
- 如果 Telegram bot 完整写入 `moltis.toml`，那么删掉 `moltis.db` 后，bot 仍可从 TOML 启动

因此，若目标是“纯 V3、纯 TOML 可恢复”，就要把 Telegram bot 配置补进 `moltis.toml` 的 `channels.telegram` 下，而不是继续依赖数据库残留。

## 当前 TOML 已知问题

当前配置口径里，有一个必须清除的旧字段：

- `tools.exec.sandbox.scope` 已删除
- 必须改为 `tools.exec.sandbox.scope_key`

如果你原来表达的是旧的 `chat` 语义，当前 V3 下通常应改成：

```toml
[tools.exec.sandbox]
scope_key = "session_key"
```

此外，当前校验还会提示两条安全告警：

- `auth.disabled = true` 且 `server.bind = 0.0.0.0`
- `tls.enabled = false` 且 `server.bind = 0.0.0.0`

这两条不是 TOML 语法错误，但意味着实例暴露方式需要你自行确认风险。

## 推荐的重置方式

如果你的目标是快速收敛到干净的 V3 状态，推荐按下面方式操作：

1. 停掉当前 Moltis 进程。
2. 先备份上文列出的最小必需项。
3. 不直接在旧目录上修修补补，改为把旧目录整体挪到带时间戳的备份目录。
4. 让系统重新生成一套新的空白 V3 目录。
5. 只恢复 `USER.md`、`PEOPLE.md`、`agents/`、`skills/` 以及你确认需要的 `moltis.toml` 内容。
6. 把 3 个 Telegram bot 的完整配置显式写进 `moltis.toml`。
7. 将任何残留的 `tools.exec.sandbox.scope` 全部改为 `scope_key`。
8. 运行 `cargo run -q -p moltis -- config check`，直到没有配置错误。
9. 启动 Moltis，确认 Telegram bot、agent 映射和 allowlist 都按 TOML 生效。

这里的关键点是：

- 不再让 `moltis.db` 承担“唯一真实来源”
- 不再继续携带旧字段残留
- 重建后的可恢复性以 TOML 和 `agents/` 目录为准

## 对当前环境的建议

对这套现有环境，更稳妥的收敛目标是：

- 把 `moltis.toml` 修成可通过 `config check`
- 把 3 个 Telegram bot 配置补齐到 `channels.telegram.<bot>`
- 保留 `USER.md`、`PEOPLE.md`、`agents/`、`skills/`
- 之后再决定是否彻底丢弃旧 `moltis.db`

这样做的好处是：

- 重置后恢复路径更短
- 启动来源更单一
- 配置行为更可预期
- 后续排障不再受旧字段污染
