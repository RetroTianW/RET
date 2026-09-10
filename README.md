# dirscan 使用手册

> 基于字典的并发目录扫描工具（Rust 实现，单文件绿色程序）

---

## 目录

1. [工具简介](#1-工具简介)
2. [环境要求](#2-环境要求)
3. [快速开始](#3-快速开始)
4. [命令行参数详解](#4-命令行参数详解)
5. [字典文件格式](#5-字典文件格式)
6. [输出解读](#6-输出解读)
7. [重定向扫描机制](#7-重定向扫描机制)
8. [常见场景示例](#8-常见场景示例)
9. [错误处理说明](#9-错误处理说明)
10. [常见问题 FAQ](#10-常见问题-faq)
11. [合法使用声明](#11-合法使用声明)

---

## 1. 工具简介

dirscan 是一款用 Rust 编写的目录扫描工具，核心特性：

| 特性 | 说明 |
|---|---|
| 字典驱动 | 从字典文件读取路径列表逐一探测 |
| 异步并发 | 基于 tokio 异步运行时，多 worker 动态取任务，自动负载均衡 |
| 状态码展示 | 实时输出每个路径的 HTTP 状态码，按类别着色 |
| 响应大小 | 每条结果附带响应字节数（Content-Length），辅助判断页面真实性 |
| 状态码过滤 | `-f` 指定只显示关心的状态码（支持精确码与区间），快速滤除 404 噪音 |
| 状态码排除 | `-e` 指定不显示的状态码（语法同 `-f`），可单独使用或与 `-f` 组合（排除优先） |
| 身份伪装 | 多 UA 轮换、Cookie 携带、HTTP Basic 认证，支持登录态与防指纹场景 |
| 代理支持 | HTTP/HTTPS/SOCKS5 代理（可多代理轮换、内嵌认证、绕过本地地址），支持环境变量与列表文件，带失败重试 |
| 重定向递归 | 遇到 301/302 标注跳转目标，并将同源目标**自动加入扫描队列**（去重） |
| 速度可控 | 通过并发数参数控制扫描速度 |
| 健壮容错 | 字典缺失、目标不可达、超时等均有明确错误提示 |

## 2. 环境要求

- **运行**：64 位 Windows（无需安装任何依赖，`dirscan.exe` 可独立运行）
- **从源码构建**（可选）：
  - Rust 1.75+（`https://rustup.rs`）
  - 构建命令：`cargo build --release`，产物位于 `target\release\dirscan.exe`

## 3. 快速开始

```powershell
# 最简用法：目标 URL + 字典文件
.\dirscan.exe -u https://example.com -w wordlist.txt

# 指定 50 并发、15 秒超时
.\dirscan.exe -u https://example.com -w wordlist.txt -t 50 -T 15

# 只显示 200 响应，滤除 404 噪音
.\dirscan.exe -u https://example.com -w wordlist.txt -f 200
```

> 提示：`wordlist.txt` 为项目自带的示例字典，可替换为你自己的字典。

## 4. 命令行参数详解

| 参数 | 缩写 | 默认值 | 说明 |
|---|---|---|---|
| `--url <URL>` | `-u` | （必填） | 目标基础 URL，支持子目录，如 `https://example.com/app/` |
| `--wordlist <FILE>` | `-w` | （必填） | 字典文件路径，每行一个路径 |
| `--threads <N>` | `-t` | `10` | 并发任务数，控制扫描速度 |
| `--timeout <SECS>` | `-T` | `10` | 单个请求的超时时间（秒），含连接超时 |
| `--filter <CODES>` | `-f` | （全部显示） | 只显示匹配的状态码，支持精确码与区间，逗号分隔，如 `200,301,400-499` |
| `--exclude <CODES>` | `-e` | （无排除） | 排除匹配的状态码不显示，语法同 `-f`（如 `404,500-599`）；可与 `-f` 组合，**排除优先** |
| `--user-agent <UA>` | | `dirscan/0.1` | 自定义 User-Agent；逗号分隔或重复传参可指定**多个**，请求间自动轮换 |
| `--cookie <C>` | | （无） | 携带 Cookie；可重复传参或分号拼接，多值自动合并为一个 Cookie 头 |
| `--auth <USER:PASS>` | | （无） | HTTP Basic 认证凭据，按第一个冒号拆分（密码可含冒号） |
| `--proxy <URL>` | | （直连） | 经代理请求，支持 `http://`、`https://`、`socks5://`、`socks5h://`，认证内嵌于 URL（`user:pass@host:port`）；逗号分隔或重复传参可指定**多个**（请求间轮换）；未指定时读取 `ALL_PROXY`/`HTTP(S)_PROXY` 环境变量 |
| `--proxy-file <FILE>` | | （无） | 代理列表文件，每行一个代理 URL（`#` 开头为注释），与 `--proxy` 可同时使用 |
| `--no-proxy <LIST>` | | 本地地址 | 代理绕过列表（逗号分隔），命中的主机直连；默认 `localhost,127.0.0.1,::1`，另读取 `NO_PROXY` 环境变量 |
| `--retries <N>` | `-r` | `1` | 请求失败（超时/连接错误）时的重试次数；多代理时重试自动切换下一个代理 |
| `--help` | `-h` | | 显示帮助 |
| `--version` | `-V` | | 显示版本 |

**参数使用要点：**

- `-u` 是否以 `/` 结尾均可，工具会自动规范：`https://ex.com/app` 等价于 `https://ex.com/app/`（基础路径自动补尾斜杠，确保字典路径正确拼接）。
- `-t` 并发数建议：目标站性能未知时从 10~20 起步；内网/自建测试环境可到 100+；过大并发可能触发目标 WAF 封禁。
- `-T` 超时对弱网目标适当调大（如 15~30），避免误报超时错误。
- `--user-agent` 可伪装为浏览器 UA；传入多个时（逗号分隔或重复传参），每个请求**轮换使用**，避免单一指纹被 WAF 识别：
  ```powershell
  # 单个 UA
  --user-agent "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36"
  # 多个 UA 轮换（注意 UA 本身不能含逗号）
  --user-agent "Mozilla/5.0 (Windows NT 10.0) Safari/537.36,Mozilla/5.0 (X11; Linux x86_64) Firefox/121.0"
  ```
- `--cookie` 用于携带登录态，两种写法等价，最终合并为一个 Cookie 头（以 `; ` 拼接）：
  ```powershell
  --cookie "SESSION=abc" --cookie "TOKEN=xyz"   # 重复传参
  --cookie "SESSION=abc; TOKEN=xyz"             # 分号拼接
  ```
- `--auth` 走 HTTP Basic 认证，凭据以 `user:pass` 形式传入；按**第一个**冒号拆分，因此密码可以包含冒号。生效后所有请求（含预检与重定向跟进）均携带 `Authorization` 头。
- 代理优先级：`--proxy`/`--proxy-file` **优先于**环境变量（`ALL_PROXY` > `HTTPS_PROXY`/`HTTP_PROXY`，按目标协议选择）。代理地址无协议前缀时默认按 `http://` 处理（如 `127.0.0.1:8080`）；代理认证直接内嵌于 URL：`http://user:pass@host:port`。多个代理时**每个请求轮换使用**，失败重试（`-r`）自动切换下一个代理，启动横幅会显示代理数量、类型、认证与否、绕过列表与来源。
- 代理对**所有请求统一生效**（预检、字典路径、重定向跟进均走代理池）；`--no-proxy` 命中的主机直连（默认含 `localhost`/`127.0.0.1`，因此扫描本地目标时配了代理也不会绕路）。
- `-f` 与 `-e` 只影响**显示**，不影响扫描本身：被过滤/排除的路径仍会被请求、计数，重定向目标也仍会递归入队跟进。二者可独立或组合使用，组合时**排除优先**（如 `-f 200-499 -e 403,404` 显示 2xx~4xx 但剔除 403/404）。
- ERR 行为：指定 `-f`（白名单）后，无状态码可匹配的 `[ERR]` 错误行同样不再显示（错误仍计入统计）；仅用 `-e`（黑名单）时 `[ERR]` 行保持显示。

## 5. 字典文件格式

**基本规则：**

```text
# 井号开头的行为注释，会被忽略
admin          # 普通路径，自动拼接为基础 URL + /admin
login
robots.txt     # 带扩展名的文件路径
.git           # 点开头的隐藏路径
/uploads/      # 带尾斜杠的目录形式（与不带斜杠视为不同 URL）
```

- 每行一个路径；
- 空行与 `#` 开头的注释行自动跳过；
- 路径开头的 `/` 会自动去掉（`/admin` 与 `admin` 等价）；
- 支持 UTF-8 编码的文本文件。

**常用字典资源**：可配合 SecLists（`https://github.com/danielmiessler/SecLists`）中的 `Discovery/Web-Content` 目录下的大型字典使用。

## 6. 输出解读

### 实时输出格式

```text
[状态码] 完整URL [响应大小] [-> 跳转目标 (附加说明)]
```

**响应大小说明：**

- 取自 HTTP 响应的 `Content-Length` 头，单位为字节（如 `[1024B]`）；
- 服务器未返回该头（如 chunked 传输）时显示 `-`；
- 大小可用于快速识别"软 404"（全站 404 页面大小高度一致）或备份文件（异常大的响应）。

**状态码颜色对照：**

| 颜色 | 状态码范围 | 含义 |
|---|---|---|
| 绿色 | 2xx | 路径存在（200 OK 等），重点关注的发现 |
| 青色 | 3xx | 重定向，会显示跳转目标 |
| 黄色 | 4xx | 客户端错误（403 禁止访问、404 不存在等） |
| 红色 | 5xx | 服务器错误 |
| 紫色 `ERR` | — | 请求失败（超时/连接失败等） |

### 结束统计示例

```text
------------------------------------------------------------
扫描完成，耗时 1.8s | 总请求: 15 | 2xx 发现: 3 | 重定向: 2 | 错误: 0
扫描完成，耗时 2.7s | 总请求: 4 | 2xx 发现: 2 | 重定向: 1 | 错误: 0 | 符合过滤并显示: 2
扫描完成，耗时 5.5s | 总请求: 2 | 2xx 发现: 1 | 重定向: 0 | 错误: 0 | 重试: 1
```

> 第二行为指定 `-f`/`-e` 过滤规则时的统计：总请求、发现数等仍是**全量**计数，末尾额外给出符合规则并显示的条数。
> 第三行的 `重试: N` 表示发生过的失败重试总次数（配置代理或弱网环境下出现，不代表最终失败——重试成功的结果正常计入统计）。

## 7. 重定向扫描机制

dirscan 对重定向采用"**标注 + 自动跟进**"策略：

1. **不自动跟随**：工具禁用了 HTTP 客户端的自动重定向，因此每一跳的状态码和 Location 都完整可见；
2. **标注目标**：输出形如 `[302] .../admin -> .../admin/login/`；
3. **自动入队**：跳转目标与目标站点**同源**（同主机、同端口）时，自动追加到扫描队列继续探测——相当于字典自动扩展，能发现字典之外的路径；
4. **严格去重**：内置已访问集合（HashSet），每个 URL 只会入队一次，重定向环（A→B→A）不会导致死循环；
5. **跨域保护**：跳转到其他域名的目标只标注、不入队，避免扫描范围失控；
6. **过滤无关性**：即使使用了 `-f` 状态码过滤把 3xx 行隐藏，同源跳转目标依然会入队扫描——过滤只影响"显示"，不影响"发现"。

**示例输出：**

```text
[302] https://httpbin.org/redirect/1 [215B] -> https://httpbin.org/get (已加入扫描队列)
[200] https://httpbin.org/get [288B]
```

## 8. 常见场景示例

```powershell
# 场景 1：常规扫描（默认 10 并发）
.\dirscan.exe -u https://example.com -w wordlist.txt

# 场景 2：扫描子目录下的路径
.\dirscan.exe -u https://example.com/app/ -w wordlist.txt

# 场景 3：低速隐蔽扫描（5 并发、长超时）
.\dirscan.exe -u https://example.com -w big-dict.txt -t 5 -T 30

# 场景 4：内网高速扫描（100 并发）
.\dirscan.exe -u http://192.168.1.10 -w wordlist.txt -t 100 -T 5

# 场景 5：伪装浏览器 UA
.\dirscan.exe -u https://example.com -w wordlist.txt `
  --user-agent "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36"

# 场景 5b：多 UA 轮换，降低被 WAF 指纹识别的概率
.\dirscan.exe -u https://example.com -w wordlist.txt `
  --user-agent "Mozilla/5.0 (Windows NT 10.0) Safari/537.36,Mozilla/5.0 (X11; Linux x86_64) Firefox/121.0"

# 场景 5c：携带登录 Cookie 扫描需要会话的路径
.\dirscan.exe -u https://example.com -w wordlist.txt --cookie "SESSION=abc" --cookie "TOKEN=xyz"

# 场景 5d：HTTP Basic 认证保护的目标
.\dirscan.exe -u https://example.com -w wordlist.txt --auth admin:S3cr3t!

# 场景 6：使用 SecLists 大字典
.\dirscan.exe -u https://example.com -w SecLists\Discovery\Web-Content\common.txt -t 30

# 场景 7：只看 200，滤除 404 噪音
.\dirscan.exe -u https://example.com -w wordlist.txt -f 200

# 场景 8：关注 200、301、302 与 403（存在但禁止访问）
.\dirscan.exe -u https://example.com -w wordlist.txt -f 200,301,302,403

# 场景 9：区间写法——只显示 2xx 与 3xx
.\dirscan.exe -u https://example.com -w wordlist.txt -f 200-299,300-399

# 场景 10：反向排除——显示除 404 外的全部结果（含 ERR、3xx 等）
.\dirscan.exe -u https://example.com -w wordlist.txt -e 404

# 场景 11：组合过滤——显示 2xx~4xx，但剔除 403 与 404
.\dirscan.exe -u https://example.com -w wordlist.txt -f 200-499 -e 403,404

# 场景 12：排除区间——显示全部结果但剔除 5xx 与 404
.\dirscan.exe -u https://example.com -w wordlist.txt -e 404,500-599

# 场景 13：经 HTTP 代理扫描（无协议前缀默认 http://）
.\dirscan.exe -u https://example.com -w wordlist.txt --proxy 127.0.0.1:8080

# 场景 14：经 SOCKS5 代理（如 ssh -D 1080 建立的动态转发）扫描
.\dirscan.exe -u https://example.com -w wordlist.txt --proxy socks5://127.0.0.1:1080

# 场景 15：带认证的代理
.\dirscan.exe -u https://example.com -w wordlist.txt --proxy "http://user:pass@10.0.0.1:8080"

# 场景 16：多代理轮换（请求间自动分配，失败重试自动切换）
.\dirscan.exe -u https://example.com -w wordlist.txt --proxy "http://10.0.0.1:8080,socks5://10.0.0.2:1080" -r 2

# 场景 17：从文件加载代理列表（每行一个，# 为注释），并绕过内网域名
.\dirscan.exe -u https://example.com -w wordlist.txt --proxy-file proxies.txt --no-proxy "*.internal.com,10.0.0.0/8"

# 场景 18：使用环境变量代理（HTTP_PROXY/HTTPS_PROXY/ALL_PROXY 自动识别）
$env:HTTPS_PROXY = "http://127.0.0.1:8080"
.\dirscan.exe -u https://example.com -w wordlist.txt
```

## 9. 错误处理说明

| 错误信息 | 原因 | 处理建议 |
|---|---|---|
| `[!] 无法读取字典文件 'xxx': ...` | 字典路径不存在或无权限 | 检查 `-w` 路径是否正确 |
| `[!] 字典文件 'xxx' 为空或只包含注释` | 字典无有效条目 | 检查字典内容 |
| `[!] 无效的目标 URL 'xxx': ...` | URL 格式错误 | 检查拼写，需含 `http://` 或 `https://` |
| `[!] 无效的状态码过滤项 'xxx'` | `-f` 表达式含非法字符或区间颠倒（如 `999-200`） | 使用 `200`、`200,301`、`400-499` 等合法形式 |
| `[!] 无效的状态码排除项 'xxx'` | `-e` 表达式含非法字符或区间颠倒 | 同上，语法与 `-f` 完全一致 |
| `[!] 无效的认证凭据 'xxx'` | `--auth` 缺少冒号或用户名为空 | 使用 `user:pass` 格式，用户名不能为空 |
| `[!] Cookie 含非法字符: ...` | Cookie 值含 HTTP 头非法字符（如中文、控制符） | 使用合法的 Cookie 键值对 |
| `[!] 不支持的协议 'ftp'` | 仅支持 http/https | 改用 http/https 地址 |
| `[!] 不支持的代理协议 'xxx'` | `--proxy` 协议前缀不合法 | 使用 `http://`、`https://`、`socks5://`、`socks5h://` 或省略前缀 |
| `[!] 无效的代理 'xxx': ...` | 代理 URL 格式错误（端口/认证非法） | 检查 URL 拼写与端口 |
| `[!] 无法读取代理文件 'xxx': ...` | `--proxy-file` 路径不存在 | 检查文件路径 |
| `[!] 目标 URL 不可达: (代理) 连接失败: ...（已配置代理，请检查代理可用性）` | 代理本身不可达（预检逐个测试全部失败） | 确认代理地址、端口、认证正确且代理进程在线 |
| `[!] 目标 URL 不可达 (无法连接/超时): ...` | 预检失败，目标无响应 | 确认目标在线、网络可达、端口正确 |
| `[ERR] ... - 超时: ...` | 单个请求超时 | 可调大 `-T`，或忽略个别慢路径 |
| `[ERR] ... - 连接失败: ...` | 连接被重置/拒绝 | 目标可能限流或防护，降低 `-t` |

> 预检失败（目标整体不可达）会直接终止程序；单个请求失败只记录错误，不影响其余扫描。

## 10. 常见问题 FAQ

**Q1：为什么很多路径都是 404，工具还有价值吗？**
有。扫描的意义就在于从大量 404 中筛出少量非 404 的路径（200/301/302/403 等）。绿色和青色结果通常最值得关注；403 往往代表路径存在但禁止访问。若嫌 404 刷屏，可用 `-f 200,301,302,403` 只显示有价值行，或反向用 `-e 404` 隐藏 404 保留其余全部（含 ERR 错误行）。

**Q1.5：响应大小显示 `-` 是什么意思？**
该响应未携带 `Content-Length` 头（常见于 chunked 传输），并非响应为空。可借助大小识别"软 404"：全站 404 页面大小几乎一致，而真实路径的大小通常不同。

**Q2：`admin` 和 `admin/` 结果为什么不同？**
许多服务器将二者视为不同资源：`/admin` 可能 301 到 `/admin/`。工具会自动跟进该跳转并去重，无需在字典中同时写两种形式。

**Q3：重定向会不会把扫描范围越带越偏？**
不会。只有与目标**同源**的跳转目标才会入队，且每个 URL 只扫描一次，扫描集合始终有限。

**Q4：并发数设多大合适？**
取决于目标承载能力。公网目标建议 10~30；遇到大量 `[ERR] 连接失败` 或持续 5xx 说明压力过大，应降低并发。内网自建环境可 100+。

**Q5：能扫描需要登录的页面吗？**
可以。会话认证用 `--cookie "SESSION=xxx"` 传入登录后的 Cookie；HTTP Basic 认证用 `--auth user:pass`。注意：表单登录（POST 账号密码到 /login）当前版本不支持，需先在浏览器登录后复制 Cookie 使用。

**Q6：结果能保存到文件吗？**
可以借助 PowerShell 重定向：
```powershell
.\dirscan.exe -u https://example.com -w wordlist.txt | Tee-Object -FilePath result.txt
```
> 注意：重定向到文件时 ANSI 颜色码会一并写入，可用编辑器查看，或使用 `result.txt` 查看时忽略乱码字符。

**Q7：支持代理吗？**
支持。`--proxy` 可指定 `http://`、`https://`、`socks5://`、`socks5h://` 代理（认证内嵌 URL：`user:pass@host:port`）；多个代理逗号分隔自动轮换；也可用 `--proxy-file` 从文件批量加载，或直接设置 `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY` 环境变量。`--no-proxy` 可指定不走代理的主机（默认绕过本地地址）。配合 `-r` 重试参数，请求失败会自动切换下一个代理重试，启动时会打印代理连接测试结果便于调试。

**Q7.5：怎么确认流量真的走了代理？**
启动时的 `[i] 代理连接测试: 通过 ... 访问目标成功 (xxxms)` 表示预检请求已成功经代理转发；`[i] 代理: N 个（轮换）[HTTP×1, SOCKS5×1] ...` 摘要行显示实际加载的代理配置。也可在代理端查看访问日志对照。

## 11. 合法使用声明

本工具仅供**已授权的安全测试**与**自有资产检查**使用，例如：

- 对自己运营的网站做路径暴露自查；
- 渗透测试中在书面授权范围内的目标评估；
- 内部安全巡检。

未经授权对他人系统进行目录扫描属于违法行为，使用者需自行承担全部法律责任。请务必遵守当地法律法规，文明、合规使用。

---

## 12. 从源码构建

```bash
# 安装 Rust 1.75+（https://rustup.rs）后执行：
git clone https://github.com/<你的用户名>/dirscan.git
cd dirscan
cargo build --release
# 产物位于 target/release/dirscan（Windows 下为 dirscan.exe）
```

## 13. 贡献指南

欢迎提交 Issue 与 Pull Request：

1. Fork 本仓库并创建特性分支：`git checkout -b feature/your-feature`；
2. 提交前运行 `cargo fmt` 与 `cargo clippy` 保证代码风格一致；
3. 保证 `cargo build --release` 与 `cargo test`（如适用）通过；
4. 发起 PR 并简要描述变更内容与动机。

## 14. 许可证

本项目基于 [MIT License](LICENSE) 开源，可自由使用、修改与分发，但需保留版权声明。使用本工具产生的一切后果由使用者自行承担。
