# 麟忆尽智 lymem · 安装与使用

麟忆尽智（lymem）是运行在麒麟等 Linux 桌面上的 **Agent 端侧长期记忆系统**：
文档与事件多源灌入（自动清洗、分块、贴库标签）、四层记忆流转（工作→情景→知识/偏好）、
偏好双通道自动提取与版本回溯、知识冲突检测仲裁、敏感信息分级与自然语言精准遗忘、
多 Agent 记忆汇聚分发，以及内置数据集的量化评测。全部数据留在本机。

安装后自带 Web 管理台：<http://127.0.0.1:8801/viewer>

---

## 安装方式一：deb 包（有 root 权限）

```bash
sudo apt install ./lymem_0.1.0_amd64.deb   # 或 sudo dpkg -i
```

安装内容：`/opt/lymem/`（服务与控制脚本）、应用菜单「麟忆尽智」、图标、
登录自启（`/etc/xdg/autostart`）、配置模板 `/etc/lymem/env`。

## 安装方式二：便携包（免 root）

```bash
tar -xzf lymem-0.1.0-linux-amd64.tar.gz
cd lymem-0.1.0-linux-amd64
./install-user.sh          # --no-autostart 可跳过开机自启
```

安装到当前用户 `~/.local/`：菜单项、图标、登录自启、`~/.local/bin/lymem` 命令。
卸载：`./uninstall-user.sh`（保留数据）。

## 开始使用

- **图形方式**：应用菜单 →「麟忆尽智」，自动启动服务并打开管理台；
- **命令方式**：

```bash
lymem open      # 启动服务并打开管理台
lymem status    # 查看运行状态
lymem stop      # 停止服务
lymem logs      # 跟踪日志
```

## 配置（可选）

配置文件：`~/.config/lymem/env`（用户级）或 `/etc/lymem/env`（系统级，deb 安装）。

| 变量 | 说明 | 默认 |
|---|---|---|
| `LYMEM_PORT` | 服务端口 | 8801 |
| `LYMEM_DATA_DIR` | 数据目录 | `~/.local/share/lymem` |
| `LYMEM_LLM_API_KEY` 等 | LLM 网关（Key/BaseURL/Model/Protocol） | 未配置 |
| `LYMEM_EMBEDDER` | `hash` 时用纯 CPU 兜底嵌入 | 麒麟端侧自动 |

不配置 LLM Key 也能使用：灌入、检索、遗忘、评测全部可用；
「检索问答」的生成直答与「会话偏好提取」将走规则兜底（功能演示建议配置）。

## 数据与迁移

全部数据（数据库 + 全文索引）在 `~/.local/share/lymem/`，备份/迁移即整目录拷贝。

## 常见问题

- **点了菜单没反应？** 终端运行 `lymem open` 查看输出；日志在 `~/.local/state/lymem/server.log`。
- **端口被占用？** 在配置文件中改 `LYMEM_PORT`。
- **为什么随桌面用户运行？** 麒麟端侧嵌入按登录用户提供（`/tmp/.kylin-ai-runtime-unix/<uid>/`），
  服务须与桌面同一用户才能使用本地嵌入与 GPU 加速；无人值守场景可在配置中指向
  kytensor HTTP 服务（`LYMEM_KYLIN_KYTENSOR_URL`）或 `LYMEM_EMBEDDER=hash`。

## REST API

服务同时提供本地 REST API（`/api/v1/*`：memories / search / ask / preferences /
knowledge / conflicts / forget / sensitive / arena / hub …）与 OpenAI 兼容嵌入代理
（`/v1/embeddings`），可供本机其他 Agent 接入。
