# dirscan User Guide

> A dictionary-based, concurrent web directory scanner written in Rust (single portable binary)

**English | [Chinese](./README.zh-CN.md)**

---

## Table of Contents

1. [Introduction](#1-introduction)
2. [Requirements](#2-requirements)
3. [Quick Start](#3-quick-start)
4. [Command-Line Options](#4-command-line-options)
5. [Wordlist Format](#5-wordlist-format)
6. [Understanding the Output](#6-understanding-the-output)
7. [Redirect Handling](#7-redirect-handling)
8. [Common Usage Examples](#8-common-usage-examples)
9. [Error Reference](#9-error-reference)
10. [Legal Use Disclaimer](#10-legal-use-disclaimer)
11. [Building from Source](#11-building-from-source)
12. [Contributing](#12-contributing)
13. [License](#13-license)

---

## 1. Introduction

dirscan is a directory scanning tool written in Rust. Key features:

| Feature               | Description                                                  |
| --------------------- | ------------------------------------------------------------ |
| Dictionary-driven     | Reads a list of paths from a wordlist file and probes them one by one |
| Async concurrency     | Built on the tokio async runtime; multiple workers pull tasks dynamically with automatic load balancing |
| Status code display   | Prints the HTTP status code of every path in real time, color-coded by class |
| Response size         | Each result includes the response size in bytes (Content-Length) to help verify real pages |
| Status code filter    | `-f` shows only the status codes you care about (exact codes and ranges supported) to quickly cut 404 noise |
| Status code exclude   | `-e` hides specific status codes (same syntax as `-f`); usable alone or combined with `-f` (exclusion wins) |
| Identity spoofing     | Multi-UA rotation, cookie support and HTTP Basic auth for logged-in sessions and anti-fingerprinting |
| Proxy support         | HTTP/HTTPS/SOCKS5 proxies (multi-proxy rotation, embedded credentials, local-address bypass), env vars and list files supported, with failure retry |
| Recursive redirects   | 301/302 responses show the redirect target, and same-origin targets are **automatically added to the scan queue** (deduplicated) |
| Controllable speed    | Scan speed is controlled via the concurrency option          |
| Robust error handling | Clear error messages for missing wordlists, unreachable targets, timeouts, etc. |

## 2. Requirements

- **Running**: 64-bit Windows (no dependencies required; `dirscan.exe` runs standalone)
- **Building from source** (optional):
  - Rust 1.75+ (`https://rustup.rs`)
  - Build command: `cargo build --release`; the binary lands at `target\release\dirscan.exe`

## 3. Quick Start

```powershell
# Minimal usage: target URL + wordlist file
.\dirscan.exe -u https://example.com -w wordlist.txt

# 50 concurrent workers, 15-second timeout
.\dirscan.exe -u https://example.com -w wordlist.txt -t 50 -T 15

# Show only 200 responses to cut 404 noise
.\dirscan.exe -u https://example.com -w wordlist.txt -f 200
```

> Tip: `wordlist.txt` is a sample wordlist shipped with the project; replace it with your own.

## 4. Command-Line Options

| Option                | Short | Default         | Description                                                  |
| --------------------- | ----- | --------------- | ------------------------------------------------------------ |
| `--url <URL>`         | `-u`  | (required)      | Base target URL; subdirectories supported, e.g. `https://example.com/app/` |
| `--wordlist <FILE>`   | `-w`  | (required)      | Path to the wordlist file, one path per line                 |
| `--threads <N>`       | `-t`  | `10`            | Number of concurrent tasks; controls scan speed              |
| `--timeout <SECS>`    | `-T`  | `10`            | Per-request timeout in seconds, including connection time    |
| `--filter <CODES>`    | `-f`  | (show all)      | Only show matching status codes; supports exact codes and ranges, comma-separated, e.g. `200,301,400-499` |
| `--exclude <CODES>`   | `-e`  | (none)          | Hide matching status codes; same syntax as `-f` (e.g. `404,500-599`); can combine with `-f`, **exclusion takes precedence** |
| `--user-agent <UA>`   |       | `dirscan/0.1`   | Custom User-Agent; comma-separated values or repeated flags specify **multiple** UAs, rotated across requests |
| `--cookie <C>`        |       | (none)          | Cookies to send; repeatable flag or semicolon-joined values, merged into a single Cookie header |
| `--auth <USER:PASS>`  |       | (none)          | HTTP Basic credentials; split on the first colon (the password may contain colons) |
| `--proxy <URL>`       |       | (direct)        | Route requests through a proxy; supports `http://`, `https://`, `socks5://`, `socks5h://`, credentials embedded in the URL (`user:pass@host:port`); comma-separated values or repeated flags specify **multiple** proxies (rotated across requests); falls back to `ALL_PROXY`/`HTTP(S)_PROXY` env vars |
| `--proxy-file <FILE>` |       | (none)          | Proxy list file, one proxy URL per line (`#` starts a comment); usable together with `--proxy` |
| `--no-proxy <LIST>`   |       | local addresses | Proxy bypass list (comma-separated); matching hosts connect directly; defaults to `localhost,127.0.0.1,::1`, also reads the `NO_PROXY` env var |
| `--retries <N>`       | `-r`  | `1`             | Retry count on request failure (timeout/connection error); with multiple proxies, retries automatically switch to the next proxy |
| `--help`              | `-h`  |                 | Show help                                                    |
| `--version`           | `-V`  |                 | Show version                                                 |

**Usage notes:**

- `-u` works with or without a trailing `/`; the tool normalizes it automatically: `https://ex.com/app` equals `https://ex.com/app/` (a trailing slash is appended to the base path so wordlist entries join correctly).

- `-t` concurrency advice: start at 10–20 for unknown targets; 100+ is fine on intranets/self-hosted test environments; excessive concurrency may trigger the target's WAF.

- Raise `-T` for slow targets (e.g. 15–30) to avoid false timeout errors.

- `--user-agent` can spoof a browser UA; when multiple UAs are supplied (comma-separated or repeated flags), each request **rotates** through them so a single fingerprint never gets flagged by WAFs:

  ```powershell
  # Single UA
  --user-agent "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36"
  # Multiple rotating UAs (note: the UAs themselves must not contain commas)
  --user-agent "Mozilla/5.0 (Windows NT 10.0) Safari/537.36,Mozilla/5.0 (X11; Linux x86_64) Firefox/121.0"
  ```

- `--cookie` carries login state; both forms below are equivalent and merge into one Cookie header (joined with `; `):

  ```powershell
  --cookie "SESSION=abc" --cookie "TOKEN=xyz"   # repeated flags
  --cookie "SESSION=abc; TOKEN=xyz"             # semicolon-joined
  ```

- `--auth` uses HTTP Basic authentication with `user:pass` credentials; split on the **first** colon, so the password may contain colons. Once set, every request (including preflight checks and redirect follow-ups) carries the `Authorization` header.

- Proxy precedence: `--proxy`/`--proxy-file` **takes precedence over** environment variables (`ALL_PROXY` > `HTTPS_PROXY`/`HTTP_PROXY`, chosen by target protocol). A proxy URL without a scheme is treated as `http://` (e.g. `127.0.0.1:8080`); credentials are embedded directly in the URL: `http://user:pass@host:port`. With multiple proxies, **each request rotates** through them; failed retries (`-r`) automatically switch to the next proxy. The startup banner prints the proxy count, types, auth status, bypass list and where they came from.

- Proxies apply to **every request uniformly** (preflight, wordlist paths and redirect follow-ups all go through the proxy pool); hosts matched by `--no-proxy` connect directly (localhost/127.0.0.1 by default, so scanning local targets never detours through a proxy).

- `-f` and `-e` only affect **display**, not the scan itself: filtered/excluded paths are still requested and counted, and redirect targets still get enqueued recursively. Use them separately or combined; when combined, **exclusion wins** (e.g. `-f 200-499 -e 403,404` shows 2xx–4xx but drops 403/404).

- ERR behavior: with `-f` (whitelist) set, `[ERR]` lines — which have no status code to match — are hidden too (errors still count in the statistics); with only `-e` (blacklist), `[ERR]` lines stay visible.

## 5. Wordlist Format

**Basic rules:**

```text
# Lines starting with # are comments and are ignored
admin          # Plain path, joined as base URL + /admin
login
robots.txt     # File path with an extension
.git           # Hidden path starting with a dot
/uploads/      # Directory form with a trailing slash (a different URL than without one)
```

- One path per line;
- Blank lines and `#` comment lines are skipped automatically;
- A leading `/` is stripped automatically (`/admin` equals `admin`);
- UTF-8 encoded text files are supported.

**Popular wordlist resources:** works well with the large wordlists under `Discovery/Web-Content` in SecLists (`https://github.com/danielmiessler/SecLists`).

## 6. Understanding the Output

> Note: the tool's runtime messages are currently printed in Chinese; the annotations below translate each one.

### Live output format

```text
[STATUS] FULL-URL [SIZE] [-> REDIRECT-TARGET (extra note)]
```

**Response size:**

- Taken from the HTTP response's `Content-Length` header, in bytes (e.g. `[1024B]`);
- Shown as `-` when the server omits that header (e.g. chunked transfers);
- Sizes help spot "soft 404s" (site-wide 404 pages share nearly the same size) or backup files (unusually large responses).

**Status code colors:**

| Color         | Status range | Meaning                                                    |
| ------------- | ------------ | ---------------------------------------------------------- |
| Green         | 2xx          | Path exists (200 OK, etc.) — the findings that matter most |
| Cyan          | 3xx          | Redirect; the target is shown                              |
| Yellow        | 4xx          | Client errors (403 Forbidden, 404 Not Found, ...)          |
| Red           | 5xx          | Server errors                                              |
| Magenta `ERR` | —            | Request failed (timeout/connection failure, ...)           |

### Final statistics example

```text
------------------------------------------------------------
Scan finished in 1.8s | Total requests: 15 | 2xx found: 3 | Redirects: 2 | Errors: 0
Scan finished in 2.7s | Total requests: 4 | 2xx found: 2 | Redirects: 1 | Errors: 0 | Matching filter shown: 2
Scan finished in 5.5s | Total requests: 2 | 2xx found: 1 | Redirects: 0 | Errors: 0 | Retries: 1
```

> The second line shows the statistics when `-f`/`-e` filters are active: totals and findings still count **everything**, with the number of matching, displayed entries appended at the end.
> The `Retries: N` in the third line counts all failed-request retries (common with proxies or weak networks) and does not mean the scan ultimately failed — successfully retried requests count normally.

## 7. Redirect Handling

dirscan follows an "**annotate + auto-follow**" strategy for redirects:

1. **No auto-following**: the HTTP client's automatic redirect following is disabled, so the status code and Location of every hop stay fully visible;
2. **Target annotation**: output like `[302] .../admin -> .../admin/login/`;
3. **Automatic enqueueing**: when the redirect target is **same-origin** with the target site (same host and port), it is automatically appended to the scan queue — effectively a self-expanding wordlist that can discover paths beyond your dictionary;
4. **Strict deduplication**: a built-in visited set (HashSet) ensures every URL is enqueued only once; redirect cycles (A→B→A) cannot loop forever;
5. **Cross-origin protection**: targets pointing to other domains are annotated but never enqueued, keeping the scan scope under control;
6. **Filter independence**: even if `-f` hides 3xx lines, same-origin redirect targets still get enqueued and scanned — filters affect only "display", never "discovery".

**Example output:**

```text
[302] https://httpbin.org/redirect/1 [215B] -> https://httpbin.org/get (added to scan queue)
[200] https://httpbin.org/get [288B]
```

## 8. Common Usage Examples

```powershell
# Scenario 1: regular scan (default 10 workers)
.\dirscan.exe -u https://example.com -w wordlist.txt

# Scenario 2: scan paths under a subdirectory
.\dirscan.exe -u https://example.com/app/ -w wordlist.txt

# Scenario 3: slow, low-profile scan (5 workers, long timeout)
.\dirscan.exe -u https://example.com -w big-dict.txt -t 5 -T 30

# Scenario 4: fast intranet scan (100 workers)
.\dirscan.exe -u http://192.168.1.10 -w wordlist.txt -t 100 -T 5

# Scenario 5: spoof a browser UA
.\dirscan.exe -u https://example.com -w wordlist.txt `
  --user-agent "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36"

# Scenario 5b: rotate multiple UAs to reduce WAF fingerprinting
.\dirscan.exe -u https://example.com -w wordlist.txt `
  --user-agent "Mozilla/5.0 (Windows NT 10.0) Safari/537.36,Mozilla/5.0 (X11; Linux x86_64) Firefox/121.0"

# Scenario 5c: scan session-protected paths with a login cookie
.\dirscan.exe -u https://example.com -w wordlist.txt --cookie "SESSION=abc" --cookie "TOKEN=xyz"

# Scenario 5d: target protected by HTTP Basic auth
.\dirscan.exe -u https://example.com -w wordlist.txt --auth admin:S3cr3t!

# Scenario 6: use a large SecLists wordlist
.\dirscan.exe -u https://example.com -w SecLists\Discovery\Web-Content\common.txt -t 30

# Scenario 7: show only 200s to cut 404 noise
.\dirscan.exe -u https://example.com -w wordlist.txt -f 200

# Scenario 8: focus on 200, 301, 302 and 403 (exists but forbidden)
.\dirscan.exe -u https://example.com -w wordlist.txt -f 200,301,302,403

# Scenario 9: range syntax — show only 2xx and 3xx
.\dirscan.exe -u https://example.com -w wordlist.txt -f 200-299,300-399

# Scenario 10: inverse exclusion — show everything except 404 (including ERR and 3xx lines)
.\dirscan.exe -u https://example.com -w wordlist.txt -e 404

# Scenario 11: combined filters — show 2xx–4xx but drop 403 and 404
.\dirscan.exe -u https://example.com -w wordlist.txt -f 200-499 -e 403,404

# Scenario 12: exclude ranges — show everything but 5xx and 404
.\dirscan.exe -u https://example.com -w wordlist.txt -e 404,500-599

# Scenario 13: scan through an HTTP proxy (no scheme prefix defaults to http://)
.\dirscan.exe -u https://example.com -w wordlist.txt --proxy 127.0.0.1:8080

# Scenario 14: scan through a SOCKS5 proxy (e.g. dynamic forwarding via ssh -D 1080)
.\dirscan.exe -u https://example.com -w wordlist.txt --proxy socks5://127.0.0.1:1080

# Scenario 15: authenticated proxy
.\dirscan.exe -u https://example.com -w wordlist.txt --proxy "http://user:pass@10.0.0.1:8080"

# Scenario 16: rotate multiple proxies (auto-assigned per request, auto-switched on retry)
.\dirscan.exe -u https://example.com -w wordlist.txt --proxy "http://10.0.0.1:8080,socks5://10.0.0.2:1080" -r 2

# Scenario 17: load proxies from a file (one per line, # for comments) and bypass intranet domains
.\dirscan.exe -u https://example.com -w wordlist.txt --proxy-file proxies.txt --no-proxy "*.internal.com,10.0.0.0/8"

# Scenario 18: use proxy environment variables (HTTP_PROXY/HTTPS_PROXY/ALL_PROXY auto-detected)
$env:HTTPS_PROXY = "http://127.0.0.1:8080"
.\dirscan.exe -u https://example.com -w wordlist.txt
```

## 9. Error Reference

> The tool prints its messages in Chinese; each real message is quoted below with its meaning.

| Message                                                      | Cause                                                        | Suggestion                                                   |
| ------------------------------------------------------------ | ------------------------------------------------------------ | ------------------------------------------------------------ |
| `[!] 无法读取字典文件 'xxx': ...` (cannot read wordlist)     | Path does not exist or no permission                         | Check the `-w` path                                          |
| `[!] 字典文件 'xxx' 为空或只包含注释` (wordlist empty or comments only) | No valid entries in the wordlist                             | Check the wordlist content                                   |
| `[!] 无效的目标 URL 'xxx': ...` (invalid target URL)         | Malformed URL                                                | Check the spelling; must include `http://` or `https://`     |
| `[!] 无效的状态码过滤项 'xxx'` (invalid status filter item)  | `-f` expression has illegal characters or a reversed range (e.g. `999-200`) | Use valid forms such as `200`, `200,301`, `400-499`          |
| `[!] 无效的状态码排除项 'xxx'` (invalid status exclude item) | Same problem in the `-e` expression                          | Identical syntax to `-f`                                     |
| `[!] 无效的认证凭据 'xxx'` (invalid credentials)             | `--auth` missing a colon or empty username                   | Use `user:pass`; the username must not be empty              |
| `[!] Cookie 含非法字符: ...` (illegal characters in cookie)  | Cookie value contains characters illegal in HTTP headers (e.g. CJK text, control chars) | Use valid cookie key-value pairs                             |
| `[!] 不支持的协议 'ftp'` (unsupported scheme)                | Only http/https are supported                                | Use an http/https URL                                        |
| `[!] 不支持的代理协议 'xxx'` (unsupported proxy scheme)      | Invalid `--proxy` scheme prefix                              | Use `http://`, `https://`, `socks5://`, `socks5h://` or omit the prefix |
| `[!] 无效的代理 'xxx': ...` (invalid proxy)                  | Malformed proxy URL (bad port/credentials)                   | Check the URL spelling and port                              |
| `[!] 无法读取代理文件 'xxx': ...` (cannot read proxy file)   | `--proxy-file` path does not exist                           | Check the file path                                          |
| `[!] 目标 URL 不可达: (代理) 连接失败: ...（已配置代理，请检查代理可用性）` (target unreachable via proxy) | The proxy itself is unreachable (all preflight tests failed) | Verify the proxy address, port, credentials and that the proxy process is running |
| `[!] 目标 URL 不可达 (无法连接/超时): ...` (target unreachable / timeout) | Preflight failed; target unresponsive                        | Confirm the target is online, reachable, and on the right port |
| `[ERR] ... - 超时: ...` (timeout)                            | A single request timed out                                   | Raise `-T`, or ignore occasional slow paths                  |
| `[ERR] ... - 连接失败: ...` (connection failed)              | Connection reset/refused                                     | The target may be rate-limiting or blocking you; lower `-t`  |

> A preflight failure (target completely unreachable) terminates the program; individual request failures are only logged and do not affect the rest of the scan.

## 10. Legal Use Disclaimer

This tool is for **authorized security testing** and **inspection of your own assets** only, for example:

- auditing your own websites for exposed paths;
- target assessment within the written scope of a penetration test;
- internal security patrols.

Running directory scans against other people's systems without authorization is illegal; users bear full legal responsibility. Always comply with local laws and use this tool lawfully and ethically.

---

## 11. Building from Source

```bash
# After installing Rust 1.75+ (https://rustup.rs):
git clone https://github.com/<your-username>/dirscan.git
cd dirscan
cargo build --release
# The binary is at target/release/dirscan (dirscan.exe on Windows)
```

## 12. Contributing

Issues and pull requests are welcome:

1. Fork the repo and create a feature branch: `git checkout -b feature/your-feature`;
2. Run `cargo fmt` and `cargo clippy` before committing to keep the style consistent;
3. Make sure `cargo build --release` and `cargo test` (where applicable) pass;
4. Open a PR briefly describing the changes and the motivation.

## 13. License

This project is open-sourced under the [MIT License](LICENSE). You are free to use, modify and distribute it, provided the copyright notice is retained. Users are solely responsible for any consequences arising from the use of this tool.

